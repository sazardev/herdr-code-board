//! The executor: it turns [`reducer::Effect`]s into herdr calls and database
//! writes, and it is the only thing in the system that starts an agent.
//!
//! Keeping dispatch in one place is what stops two processes from racing to
//! occupy the same pane: the TUI and the CLI hooks only ever set a card to
//! `ready`, and the single engine instance picks it up from there.

use std::sync::Arc;

use anyhow::{Context, Result};

use super::placement::{self, Target};
use super::reducer::{self, Effect, Input};
use crate::agents;
use crate::config::Config;
use crate::herdr::HerdrApi;
use crate::model::{Action, AgentStatus, Binding, Card, Column, Repo};
use crate::store::Store;

/// Herdr returns this when an agent is up but sitting on a startup dialog. The
/// name stays usable, so it is not a dispatch failure.
const AGENT_NOT_READY: &str = "agent_not_ready";

pub struct Executor {
    pub store: Store,
    pub herdr: Arc<dyn HerdrApi>,
    pub config: Config,
}

impl Executor {
    pub fn new(store: Store, herdr: Arc<dyn HerdrApi>, config: Config) -> Self {
        Self {
            store,
            herdr,
            config,
        }
    }

    // ---- inputs -----------------------------------------------------------

    /// Feed one agent status change, addressed by the pane it happened in.
    pub fn on_agent_status(&mut self, pane_id: &str, status: AgentStatus) -> Result<()> {
        let Some(card) = self.store.card_for_pane(pane_id)? else {
            return Ok(());
        };
        self.feed(&card, Input::AgentStatus(status))
    }

    /// A pane or its workspace went away.
    pub fn on_pane_gone(&mut self, pane_id: &str) -> Result<()> {
        let Some(card) = self.store.card_for_pane(pane_id)? else {
            return Ok(());
        };
        self.feed(&card, Input::PaneGone)
    }

    pub fn on_workspace_gone(&mut self, workspace_id: &str) -> Result<()> {
        let affected: Vec<Card> = self
            .store
            .live_cards()?
            .into_iter()
            .filter(|c| c.binding.workspace_id.as_deref() == Some(workspace_id))
            .collect();
        for card in affected {
            self.feed(&card, Input::PaneGone)?;
        }
        Ok(())
    }

    /// Run the timed rules for every live card.
    pub fn tick(&mut self, now: i64) -> Result<()> {
        for card in self.store.live_cards()? {
            self.feed(&card, Input::Tick { now })?;
        }
        Ok(())
    }

    /// The next unix time a timed rule could fire, across all live cards.
    pub fn next_deadline(&self) -> Result<Option<i64>> {
        let mut best: Option<i64> = None;
        for card in self.store.live_cards()? {
            let rules = self
                .store
                .rules_for_card(&card.id, card.repo_id.as_deref())?;
            if let Some(d) = reducer::next_deadline(&card, &rules) {
                best = Some(best.map_or(d, |b: i64| b.min(d)));
            }
        }
        Ok(best)
    }

    fn feed(&mut self, card: &Card, input: Input) -> Result<()> {
        let rules = self
            .store
            .rules_for_card(&card.id, card.repo_id.as_deref())?;
        let effects = reducer::step(card, &rules, &input);
        self.apply(card, effects)
    }

    // ---- effects ----------------------------------------------------------

    fn apply(&mut self, card: &Card, effects: Vec<Effect>) -> Result<()> {
        for effect in effects {
            match effect {
                Effect::Lane(lane) => {
                    if self.store.set_lane(&card.id, lane)? {
                        self.store
                            .log_event("lane", Some(&card.id), Some(lane.as_str()))?;
                    }
                }
                Effect::ClearBinding => self.store.clear_binding(&card.id)?,
                Effect::FinishRun { outcome, detail } => {
                    self.store
                        .finish_open_run(&card.id, &outcome, detail.as_deref())?
                }
                Effect::Log { kind, detail } => {
                    self.store
                        .log_event(&kind, Some(&card.id), detail.as_deref())?
                }
                Effect::Fire { rule_id, action } => {
                    // Consuming the budget first is what makes a rule fire at most
                    // `max_fires` times even if the executor later errors out.
                    if !self.store.try_consume_rule(&rule_id)? {
                        continue;
                    }
                    // Re-read: earlier effects in this batch may have moved the card.
                    let current = self
                        .store
                        .get_card(&card.id)?
                        .unwrap_or_else(|| card.clone());
                    if let Err(e) = self.run_action(&current, &action) {
                        self.store.log_event(
                            "rule_error",
                            Some(&card.id),
                            Some(&format!("{action:?}: {e}")),
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    fn run_action(&mut self, card: &Card, action: &Action) -> Result<()> {
        match action {
            Action::Enqueue { cards } => {
                for needle in cards {
                    match self.store.resolve_card(needle)? {
                        Some(next) if next.column.is_live() => {
                            self.store.log_event(
                                "enqueue_skipped",
                                Some(&next.id),
                                Some("already running"),
                            )?;
                        }
                        Some(next) => {
                            self.store.reset_rule_fires(&next.id)?;
                            self.store.set_lane(&next.id, Column::Ready)?;
                            self.store.log_event(
                                "enqueued",
                                Some(&next.id),
                                Some(&format!("by {}", card.id)),
                            )?;
                        }
                        None => {
                            self.store.log_event(
                                "enqueue_missing",
                                Some(&card.id),
                                Some(needle),
                            )?;
                        }
                    }
                }
                Ok(())
            }

            Action::Prompt { text } => {
                let target = self.agent_target(card)?;
                self.herdr.prompt_agent(&target, text)?;
                self.store.set_prompt_sent(&card.id, true)?;
                self.store
                    .log_event("prompted", Some(&card.id), Some(text))?;
                Ok(())
            }

            Action::Answer { choice } => self.answer_dialog(card, *choice),

            Action::SendKeys { keys } => {
                let target = self.agent_target(card)?;
                self.herdr.send_keys(&target, keys)?;
                self.store
                    .log_event("send_keys", Some(&card.id), Some(&keys.join(" ")))?;
                Ok(())
            }

            Action::Notify { title, body } => {
                if self.config.notifications {
                    self.herdr.notify(title, body.as_deref())?;
                }
                Ok(())
            }

            Action::Retry => {
                self.store.clear_binding(&card.id)?;
                self.store.set_lane(&card.id, Column::Ready)?;
                Ok(())
            }

            Action::Cancel => {
                self.store
                    .finish_open_run(&card.id, "cancelled", Some("by rule"))?;
                self.store.set_lane(&card.id, Column::Cancelled)?;
                Ok(())
            }

            Action::ClosePane => {
                if let Some(pane) = &card.binding.pane_id {
                    self.herdr.close_pane(pane)?;
                }
                self.store.clear_binding(&card.id)?;
                Ok(())
            }
        }
    }

    /// Answer a blocked approval dialog on the card's behalf.
    ///
    /// This types into a pane the user may be watching, so it needs two
    /// independent opt-ins: the global `allow_auto_answer` and the card's own
    /// flag. What the dialog said is recorded before answering, so there is an
    /// audit trail of what was approved.
    fn answer_dialog(&mut self, card: &Card, choice: u32) -> Result<()> {
        if !self.config.allow_auto_answer {
            self.store.log_event(
                "auto_answer_refused",
                Some(&card.id),
                Some("allow_auto_answer is false in config.toml"),
            )?;
            return Ok(());
        }
        if !card.auto_answer {
            self.store.log_event(
                "auto_answer_refused",
                Some(&card.id),
                Some("this card does not opt into auto answering"),
            )?;
            return Ok(());
        }
        let target = self.agent_target(card)?;

        // `visible` is the cheap read source; `recent` would repaint the pane.
        if let Some(pane) = &card.binding.pane_id {
            if let Ok(screen) = self.herdr.read_visible(pane, 40) {
                let excerpt: String = screen.lines().rev().take(12).collect::<Vec<_>>().join("\n");
                self.store
                    .note_open_run(&card.id, &format!("dialog before auto-answer:\n{excerpt}"))?;
            }
        }

        let key = choice.to_string();
        self.herdr.send_keys(&target, std::slice::from_ref(&key))?;
        self.store
            .note_open_run(&card.id, &format!("auto-answered choice {choice}"))?;
        self.store
            .log_event("auto_answered", Some(&card.id), Some(&key))?;
        Ok(())
    }

    /// Agent commands take a pane id that currently hosts an agent. Using the pane
    /// rather than the name avoids depending on herdr echoing the name back.
    fn agent_target(&self, card: &Card) -> Result<String> {
        card.binding
            .pane_id
            .clone()
            .with_context(|| format!("card {} owns no pane", card.id))
    }

    // ---- dispatch ---------------------------------------------------------

    /// Start every `ready` card that fits within its repo's concurrency budget.
    /// Returns how many were dispatched.
    pub fn dispatch_ready(&mut self) -> Result<usize> {
        let ready = self.store.cards_in(Column::Ready)?;
        let mut started = 0;
        for card in ready {
            if !self.has_capacity(&card)? {
                continue;
            }
            match self.dispatch(&card) {
                Ok(true) => started += 1,
                Ok(false) => {}
                Err(e) => {
                    self.fail_card(&card, &format!("{e:#}"))?;
                }
            }
        }
        Ok(started)
    }

    fn budget_for(&self, repo: Option<&Repo>) -> u32 {
        repo.map(|r| r.max_parallel)
            .filter(|n| *n > 0)
            .unwrap_or(self.config.default_max_parallel)
    }

    fn has_capacity(&self, card: &Card) -> Result<bool> {
        let repo = match &card.repo_id {
            Some(id) => self.store.get_repo(id)?,
            None => None,
        };
        let budget = self.budget_for(repo.as_ref());
        let live = self
            .store
            .live_cards()?
            .into_iter()
            .filter(|c| c.repo_id == card.repo_id)
            .count() as u32;
        Ok(live < budget)
    }

    /// Where a card's agent should run. Cards without a repo run from `$HOME`.
    fn repo_context(&self, card: &Card) -> Result<(String, String)> {
        if let Some(id) = &card.repo_id {
            if let Some(repo) = self.store.get_repo(id)? {
                return Ok((repo.path, repo.name));
            }
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        Ok((home, "board".into()))
    }

    fn dispatch(&mut self, card: &Card) -> Result<bool> {
        let (repo_path, repo_name) = self.repo_context(card)?;
        let slug = card.slug();

        // Count the attempt before anything can fail. Otherwise a card that dies
        // during placement never burns retry budget and the engine loops on it.
        let attempt = self.store.mark_dispatched(&card.id)?;
        let run = self.store.start_run(&card.id, attempt)?;

        let target: Target = placement::resolve(
            self.herdr.as_ref(),
            &repo_path,
            &repo_name,
            &card.placement,
            &slug,
        )?;

        let binding = Binding {
            workspace_id: Some(target.workspace_id.clone()),
            tab_id: target.tab_id.clone(),
            pane_id: Some(target.pane_id.clone()),
            agent_name: Some(slug.clone()),
            worktree_path: target.worktree_path.clone(),
        };
        self.store.set_binding(&card.id, &binding)?;
        self.store.set_prompt_sent(&card.id, false)?;
        self.store.set_lane(&card.id, Column::Running)?;

        let args = self.agent_args(card)?;
        match self
            .herdr
            .start_agent(&slug, &card.agent_kind, &target.pane_id, &args)
        {
            Ok(()) => {}
            Err(e) if e.to_string().contains(AGENT_NOT_READY) => {
                // The agent is up but sitting on a startup dialog. Park the card in
                // `blocked` and let the status events drive it from here.
                self.store.set_lane(&card.id, Column::Blocked)?;
                self.store.log_event(
                    "agent_not_ready",
                    Some(&card.id),
                    Some("started but blocked on a startup dialog"),
                )?;
                return Ok(true);
            }
            Err(e) => {
                self.store
                    .finish_run(&run.id, "failed", Some(&format!("{e:#}")))?;
                return Err(e).context("starting the agent");
            }
        }

        // A named pane makes the card findable in herdr's own UI.
        let _ = self.herdr.rename_pane(&target.pane_id, &card.title);

        if !card.prompt.trim().is_empty() {
            self.herdr
                .prompt_agent(&target.pane_id, &card.prompt)
                .context("sending the card's prompt")?;
        }
        // The handover is complete either way: from here an idle agent means the
        // card's turn ended, not that it is still booting.
        self.store.set_prompt_sent(&card.id, true)?;

        self.store.set_error(&card.id, None)?;
        self.store
            .log_event("dispatched", Some(&card.id), Some(&target.pane_id))?;
        Ok(true)
    }

    fn agent_args(&self, card: &Card) -> Result<Vec<String>> {
        let mut args = Vec::new();
        if let Some(model) = card.model.as_deref().filter(|m| !m.trim().is_empty()) {
            let (model_args, mapped) =
                agents::model_args(&self.config.model_flags, &card.agent_kind, model);
            if !mapped {
                self.store.log_event(
                    "model_flag_guessed",
                    Some(&card.id),
                    Some(&format!(
                        "no model_flags entry for {}; assuming {}",
                        card.agent_kind,
                        agents::FALLBACK_MODEL_FLAG
                    )),
                )?;
            }
            args.extend(model_args);
        }
        args.extend(card.extra_args.iter().cloned());
        Ok(args)
    }

    fn fail_card(&mut self, card: &Card, error: &str) -> Result<()> {
        self.store.set_error(&card.id, Some(error))?;
        self.store
            .finish_open_run(&card.id, "failed", Some(error))?;
        self.store.clear_binding(&card.id)?;

        // Re-read: `card` predates this dispatch, so its attempt count is stale.
        let card = &self
            .store
            .get_card(&card.id)?
            .unwrap_or_else(|| card.clone());
        if card.attempts <= card.max_retries {
            self.store.set_lane(&card.id, Column::Ready)?;
            self.store.log_event("retry", Some(&card.id), Some(error))?;
            return Ok(());
        }
        self.store.set_lane(&card.id, Column::Failed)?;
        self.store
            .log_event("failed", Some(&card.id), Some(error))?;

        // Failure rules still deserve to run.
        let rules = self
            .store
            .rules_for_card(&card.id, card.repo_id.as_deref())?;
        let failed = self
            .store
            .get_card(&card.id)?
            .unwrap_or_else(|| card.clone());
        for rule in rules
            .into_iter()
            .filter(|r| r.trigger == crate::model::Trigger::Failed)
        {
            if self.store.try_consume_rule(&rule.id)? {
                let _ = self.run_action(&failed, &rule.action);
            }
        }
        Ok(())
    }
}
