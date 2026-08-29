//! The new/edit card form.
//!
//! Kept as plain state with no terminal dependency so the field behaviour can be
//! tested directly.

use crate::model::{Card, Placement, Repo, SplitDirection};
use crate::store::cards::NewCard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Title,
    Prompt,
    Repo,
    Agent,
    Model,
    Placement,
    /// Worktree only: the branch to cut.
    Branch,
    /// Worktree only: what to cut it from.
    Base,
    Tags,
    Args,
    Flags,
}

impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Prompt => "prompt",
            Field::Repo => "repo",
            Field::Agent => "agent",
            Field::Model => "model",
            Field::Placement => "placement",
            Field::Branch => "branch",
            Field::Base => "from",
            Field::Tags => "tags",
            Field::Args => "args",
            Field::Flags => "flags",
        }
    }

    /// Text fields take typed characters; the rest cycle with left/right.
    pub fn is_text(self) -> bool {
        matches!(
            self,
            Field::Title | Field::Prompt | Field::Model | Field::Tags | Field::Args | Field::Branch
        )
    }
}

/// Placement options offered in the form, in cycle order.
const PLACEMENTS: [&str; 5] = ["split", "reuse", "tab", "workspace", "worktree"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    Start,
    Review,
    AutoAnswer,
}

impl Flag {
    pub const ALL: [Flag; 3] = [Flag::Start, Flag::Review, Flag::AutoAnswer];

    pub fn label(self) -> &'static str {
        match self {
            Flag::Start => "start now",
            Flag::Review => "needs review",
            Flag::AutoAnswer => "auto-answer",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Form {
    /// `Some` when editing an existing card.
    pub editing: Option<String>,
    pub field: usize,
    pub title: String,
    pub prompt: String,
    pub model: String,
    pub tags: String,
    pub args: String,
    /// Index into `repos`, where 0 means "no repo".
    pub repo: usize,
    pub agent: usize,
    pub placement: usize,
    /// Branch to cut, for a worktree card. `{card}` expands to the card slug.
    pub branch: String,
    /// Index into `bases`, the repo's real branches.
    pub base: usize,
    /// The chosen repo's branches, newest first. Filled in by the run loop,
    /// because the form itself does no I/O.
    pub bases: Vec<String>,
    pub flag: usize,
    pub start: bool,
    pub review: bool,
    pub auto_answer: bool,
    /// Repo choices, with a leading "-" entry for global cards.
    pub repos: Vec<(String, String)>,
    pub agents: Vec<String>,
}

impl Form {
    pub fn new(repos: &[Repo], agents: Vec<String>, default_agent: &str) -> Self {
        let mut choices = vec![("-".to_string(), String::new())];
        choices.extend(repos.iter().map(|r| (r.name.clone(), r.id.clone())));
        let agent = agents.iter().position(|a| a == default_agent).unwrap_or(0);
        Self {
            editing: None,
            field: 0,
            title: String::new(),
            prompt: String::new(),
            model: String::new(),
            tags: String::new(),
            args: String::new(),
            repo: 0,
            agent,
            placement: 0,
            branch: "board/{card}".to_string(),
            base: 0,
            bases: Vec::new(),
            flag: 0,
            start: true,
            review: false,
            auto_answer: false,
            repos: choices,
            agents,
        }
    }

    pub fn edit(card: &Card, repos: &[Repo], agents: Vec<String>) -> Self {
        let mut form = Self::new(repos, agents, &card.agent_kind);
        form.editing = Some(card.id.clone());
        form.title = card.title.clone();
        form.prompt = card.prompt.clone();
        form.model = card.model.clone().unwrap_or_default();
        form.tags = card.tags.join(", ");
        form.args = card.args_line();
        form.repo = card
            .repo_id
            .as_ref()
            .and_then(|id| form.repos.iter().position(|(_, rid)| rid == id))
            .unwrap_or(0);
        form.agent = form
            .agents
            .iter()
            .position(|a| a == &card.agent_kind)
            .unwrap_or(0);
        form.placement = PLACEMENTS
            .iter()
            .position(|p| *p == placement_name(&card.placement))
            .unwrap_or(0);
        if let Placement::Worktree { branch, base } = &card.placement {
            form.branch = branch.clone();
            if let Some(b) = base {
                form.bases = vec![b.clone()];
            }
        }
        form.start = false;
        form.review = !card.auto_complete;
        form.auto_answer = card.auto_answer;
        form
    }

    /// The fields on screen right now. Branch and base only exist for a worktree
    /// card, because for any other placement they would be dead controls.
    pub fn fields(&self) -> Vec<Field> {
        let mut out = vec![
            Field::Title,
            Field::Prompt,
            Field::Repo,
            Field::Agent,
            Field::Model,
            Field::Placement,
        ];
        if self.placement_name() == "worktree" {
            out.push(Field::Branch);
            out.push(Field::Base);
        }
        out.extend([Field::Tags, Field::Args, Field::Flags]);
        out
    }

    pub fn current(&self) -> Field {
        let fields = self.fields();
        fields[self.field.min(fields.len() - 1)]
    }

    pub fn next_field(&mut self) {
        let len = self.fields().len();
        self.field = (self.field + 1) % len;
    }

    pub fn prev_field(&mut self) {
        let len = self.fields().len();
        self.field = (self.field + len - 1) % len;
    }

    /// Offer these as the `from` choices, keeping the current pick if it survives.
    pub fn set_bases(&mut self, bases: Vec<String>) {
        let keep = self.bases.get(self.base).cloned();
        self.bases = bases;
        self.base = keep
            .and_then(|b| self.bases.iter().position(|x| *x == b))
            .unwrap_or(0);
    }

    pub fn base_name(&self) -> Option<&str> {
        self.bases.get(self.base).map(String::as_str)
    }

    pub fn push(&mut self, ch: char) {
        match self.current() {
            Field::Title => self.title.push(ch),
            Field::Prompt => self.prompt.push(ch),
            Field::Model => self.model.push(ch),
            Field::Tags => self.tags.push(ch),
            Field::Args => self.args.push(ch),
            Field::Branch => self.branch.push(ch),
            _ => {}
        }
    }

    pub fn backspace(&mut self) {
        match self.current() {
            Field::Title => self.title.pop(),
            Field::Prompt => self.prompt.pop(),
            Field::Model => self.model.pop(),
            Field::Tags => self.tags.pop(),
            Field::Args => self.args.pop(),
            Field::Branch => self.branch.pop(),
            _ => None,
        };
    }

    /// Left/right on a choice field, or toggling the highlighted flag.
    pub fn cycle(&mut self, forward: bool) {
        let step = |i: usize, len: usize| {
            if len == 0 {
                0
            } else if forward {
                (i + 1) % len
            } else {
                (i + len - 1) % len
            }
        };
        match self.current() {
            Field::Repo => self.repo = step(self.repo, self.repos.len()),
            Field::Agent => self.agent = step(self.agent, self.agents.len()),
            Field::Placement => {
                self.placement = step(self.placement, PLACEMENTS.len());
                // The field list just changed shape; keep the cursor in range.
                self.field = self.field.min(self.fields().len() - 1);
            }
            Field::Base => self.base = step(self.base, self.bases.len()),
            Field::Flags => self.flag = step(self.flag, Flag::ALL.len()),
            _ => {}
        }
    }

    pub fn toggle(&mut self) {
        if self.current() != Field::Flags {
            return;
        }
        match Flag::ALL[self.flag] {
            Flag::Start => self.start = !self.start,
            Flag::Review => self.review = !self.review,
            Flag::AutoAnswer => self.auto_answer = !self.auto_answer,
        }
    }

    pub fn flag_value(&self, flag: Flag) -> bool {
        match flag {
            Flag::Start => self.start,
            Flag::Review => self.review,
            Flag::AutoAnswer => self.auto_answer,
        }
    }

    pub fn placement_name(&self) -> &'static str {
        PLACEMENTS[self.placement.min(PLACEMENTS.len() - 1)]
    }

    pub fn repo_id(&self) -> Option<String> {
        self.repos
            .get(self.repo)
            .map(|(_, id)| id.clone())
            .filter(|id| !id.is_empty())
    }

    pub fn agent_kind(&self) -> String {
        self.agents
            .get(self.agent)
            .cloned()
            .unwrap_or_else(|| "claude".to_string())
    }

    fn placement_value(&self) -> Placement {
        match self.placement_name() {
            "reuse" => Placement::Reuse,
            "tab" => Placement::NewTab,
            "workspace" => Placement::NewWorkspace,
            "worktree" => Placement::Worktree {
                branch: if self.branch.trim().is_empty() {
                    "board/{card}".into()
                } else {
                    self.branch.trim().to_string()
                },
                base: self.base_name().map(str::to_string),
            },
            _ => Placement::Split {
                direction: None,
                ratio: None,
            },
        }
    }

    fn split_list(raw: &str) -> Vec<String> {
        raw.split([',', ' '])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// The form as a new card. `None` when the title is still empty.
    pub fn to_new_card(&self) -> Option<NewCard> {
        let title = self.title.trim();
        if title.is_empty() {
            return None;
        }
        Some(NewCard {
            key: None,
            title: title.to_string(),
            prompt: self.prompt.clone(),
            repo_id: self.repo_id(),
            session: crate::session::current_name(),
            tags: Self::split_list(&self.tags),
            agent_kind: self.agent_kind(),
            model: Some(self.model.trim().to_string()).filter(|m| !m.is_empty()),
            extra_args: Self::split_list(&self.args),
            placement: self.placement_value(),
            column: if self.start {
                crate::model::Column::Ready
            } else {
                crate::model::Column::Backlog
            },
            priority: 0,
            auto_complete: !self.review,
            auto_answer: self.auto_answer,
            max_retries: 0,
        })
    }

    /// Apply the form onto the card it is editing.
    pub fn apply_to(&self, card: &mut Card) {
        card.title = self.title.trim().to_string();
        card.prompt = self.prompt.clone();
        card.repo_id = self.repo_id();
        card.tags = Self::split_list(&self.tags);
        card.agent_kind = self.agent_kind();
        card.model = Some(self.model.trim().to_string()).filter(|m| !m.is_empty());
        card.extra_args = Self::split_list(&self.args);
        card.placement = self.placement_value();
        card.auto_complete = !self.review;
        card.auto_answer = self.auto_answer;
    }
}

pub fn placement_name(placement: &Placement) -> &'static str {
    match placement {
        Placement::Reuse => "reuse",
        Placement::Split { .. } => "split",
        Placement::NewTab => "tab",
        Placement::NewWorkspace => "workspace",
        Placement::Worktree { .. } => "worktree",
    }
}

/// A one-line summary of a placement, for the detail panel.
pub fn placement_summary(placement: &Placement) -> String {
    match placement {
        Placement::Reuse => "reuse a free pane".into(),
        Placement::Split { direction, ratio } => {
            let dir = match direction {
                Some(SplitDirection::Right) => "right",
                Some(SplitDirection::Down) => "down",
                None => "auto",
            };
            match ratio {
                Some(r) => format!("split {dir} at {r}"),
                None => format!("split {dir}"),
            }
        }
        Placement::NewTab => "new tab".into(),
        Placement::NewWorkspace => "new workspace".into(),
        Placement::Worktree { branch, base } => match base {
            Some(b) => format!("worktree {branch} from {b}"),
            None => format!("worktree {branch}"),
        },
    }
}

impl Card {
    /// Extra agent args as one editable line.
    pub fn args_line(&self) -> String {
        self.extra_args.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Column;

    fn repos() -> Vec<Repo> {
        vec![Repo {
            id: "R1".into(),
            name: "erp".into(),
            path: "/repo/erp".into(),
            tags: vec![],
            max_parallel: 2,
            default_agent: None,
            default_model: None,
        }]
    }

    fn form() -> Form {
        Form::new(&repos(), vec!["claude".into(), "codex".into()], "codex")
    }

    #[test]
    fn the_default_agent_is_preselected() {
        assert_eq!(form().agent_kind(), "codex");
    }

    #[test]
    fn typing_only_lands_in_text_fields() {
        let mut f = form();
        f.push('h');
        f.push('i');
        assert_eq!(f.title, "hi");

        // Move to the agent field, which is a chooser, not a text box.
        f.field = f.fields().iter().position(|x| *x == Field::Agent).unwrap();
        f.push('z');
        assert_eq!(f.agent_kind(), "codex", "typing must not corrupt a chooser");
    }

    #[test]
    fn choosers_wrap_in_both_directions() {
        let mut f = form();
        f.field = f.fields().iter().position(|x| *x == Field::Agent).unwrap();
        f.cycle(true);
        assert_eq!(f.agent_kind(), "claude");
        f.cycle(false);
        assert_eq!(f.agent_kind(), "codex");
        f.cycle(false);
        assert_eq!(f.agent_kind(), "claude", "cycling back wraps around");
    }

    #[test]
    fn flags_toggle_independently() {
        let mut f = form();
        f.field = f.fields().iter().position(|x| *x == Field::Flags).unwrap();
        assert!(f.start);
        f.toggle();
        assert!(!f.start);
        f.cycle(true);
        f.toggle();
        assert!(f.review);
        assert!(!f.auto_answer);
    }

    #[test]
    fn branch_and_base_appear_only_for_a_worktree_card() {
        let mut f = form();
        assert!(!f.fields().contains(&Field::Branch));
        assert!(!f.fields().contains(&Field::Base));

        f.placement = PLACEMENTS.iter().position(|p| *p == "worktree").unwrap();
        assert!(f.fields().contains(&Field::Branch));
        assert!(f.fields().contains(&Field::Base));
    }

    #[test]
    fn shrinking_the_field_list_does_not_strand_the_cursor() {
        let mut f = form();
        f.placement = PLACEMENTS.iter().position(|p| *p == "worktree").unwrap();
        f.field = f.fields().len() - 1;
        let last = f.current();

        // Cycle the placement away from worktree: two fields vanish underneath.
        f.field = f
            .fields()
            .iter()
            .position(|x| *x == Field::Placement)
            .unwrap();
        f.cycle(true);
        assert!(f.field < f.fields().len());
        let _ = last;
    }

    #[test]
    fn the_base_chooser_keeps_your_pick_when_the_branch_list_is_reloaded() {
        let mut f = form();
        f.set_bases(vec!["main".into(), "develop".into(), "old".into()]);
        f.placement = PLACEMENTS.iter().position(|p| *p == "worktree").unwrap();
        f.field = f.fields().iter().position(|x| *x == Field::Base).unwrap();
        f.cycle(true);
        assert_eq!(f.base_name(), Some("develop"));

        // A rescan reorders the branches; the chosen one must still be chosen.
        f.set_bases(vec!["develop".into(), "main".into()]);
        assert_eq!(f.base_name(), Some("develop"));

        // And a branch that disappeared falls back to the first.
        f.set_bases(vec!["main".into()]);
        assert_eq!(f.base_name(), Some("main"));
    }

    #[test]
    fn a_worktree_card_carries_its_branch_and_base() {
        let mut f = form();
        f.title = "t".into();
        f.placement = PLACEMENTS.iter().position(|p| *p == "worktree").unwrap();
        f.branch = "board/try".into();
        f.set_bases(vec!["develop".into()]);
        assert_eq!(
            f.to_new_card().unwrap().placement,
            Placement::Worktree {
                branch: "board/try".into(),
                base: Some("develop".into())
            }
        );
    }

    #[test]
    fn an_empty_branch_falls_back_to_the_default_pattern() {
        let mut f = form();
        f.title = "t".into();
        f.placement = PLACEMENTS.iter().position(|p| *p == "worktree").unwrap();
        f.branch = "   ".into();
        assert_eq!(
            f.to_new_card().unwrap().placement,
            Placement::Worktree {
                branch: "board/{card}".into(),
                base: None
            }
        );
    }

    #[test]
    fn a_form_without_a_title_produces_no_card() {
        let mut f = form();
        assert!(f.to_new_card().is_none());
        f.title = "   ".into();
        assert!(f.to_new_card().is_none());
    }

    #[test]
    fn a_filled_form_becomes_a_ready_card() {
        let mut f = form();
        f.title = "  Review the diff  ".into();
        f.prompt = "look at it".into();
        f.model = " opus ".into();
        f.tags = "review, urgent".into();
        f.args = "--permission-mode plan".into();
        f.repo = 1;
        f.placement = PLACEMENTS.iter().position(|p| *p == "worktree").unwrap();

        let card = f.to_new_card().unwrap();
        assert_eq!(card.title, "Review the diff");
        assert_eq!(card.model.as_deref(), Some("opus"));
        assert_eq!(card.tags, vec!["review", "urgent"]);
        assert_eq!(card.extra_args, vec!["--permission-mode", "plan"]);
        assert_eq!(card.repo_id.as_deref(), Some("R1"));
        assert_eq!(card.column, Column::Ready);
        assert!(card.auto_complete);
        assert!(matches!(card.placement, Placement::Worktree { .. }));
    }

    #[test]
    fn an_empty_model_is_stored_as_none_not_as_an_empty_string() {
        let mut f = form();
        f.title = "t".into();
        f.model = "   ".into();
        assert_eq!(f.to_new_card().unwrap().model, None);
    }

    #[test]
    fn editing_round_trips_a_card_through_the_form() {
        let store = crate::store::Store::open_in_memory().unwrap();
        let original = store
            .create_card(&NewCard {
                prompt: "p".into(),
                tags: vec!["a".into(), "b".into()],
                model: Some("opus".into()),
                extra_args: vec!["--x".into()],
                placement: Placement::NewTab,
                auto_complete: false,
                auto_answer: true,
                ..NewCard::new("orig", "codex")
            })
            .unwrap();

        let f = Form::edit(&original, &repos(), vec!["claude".into(), "codex".into()]);
        assert_eq!(f.placement_name(), "tab");
        assert!(f.review);
        assert!(f.auto_answer);
        assert!(!f.start, "editing must not silently requeue the card");

        let mut copy = original.clone();
        f.apply_to(&mut copy);
        assert_eq!(copy.title, original.title);
        assert_eq!(copy.tags, original.tags);
        assert_eq!(copy.extra_args, original.extra_args);
        assert_eq!(copy.model, original.model);
        assert_eq!(copy.placement, original.placement);
        assert_eq!(copy.auto_complete, original.auto_complete);
    }
}
