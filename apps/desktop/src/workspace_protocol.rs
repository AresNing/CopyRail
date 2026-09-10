//! Shared, bounded messages between the rail and its owned auxiliary window.
use paste_domain::ClipItem;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceContent {
    #[default]
    Closed,
    Settings {
        tab: String,
    },
    Preview {
        clip: Box<ClipItem>,
    },
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct WorkspaceSnapshot {
    pub revision: u64,
    pub content: WorkspaceContent,
}

impl WorkspaceSnapshot {
    pub fn replace(&mut self, content: WorkspaceContent) {
        self.revision = self.revision.wrapping_add(1);
        self.content = content;
    }
    pub fn accepts_ready(&self, revision: u64) -> bool {
        self.revision == revision && !matches!(self.content, WorkspaceContent::Closed)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WorkspaceKey {
    pub key: String,
    pub shift: bool,
    pub meta: bool,
}

impl WorkspaceKey {
    pub fn valid(&self) -> bool {
        matches!(
            self.key.as_str(),
            "ArrowLeft"
                | "ArrowRight"
                | "Home"
                | "End"
                | "Enter"
                | " "
                | "Escape"
                | "c"
                | "C"
                | "o"
                | "O"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_render_cannot_reopen_a_closed_or_replaced_workspace() {
        let mut state = WorkspaceSnapshot::default();
        state.replace(WorkspaceContent::Settings {
            tab: "general".into(),
        });
        let old = state.revision;
        assert!(state.accepts_ready(old));
        state.replace(WorkspaceContent::Closed);
        assert!(!state.accepts_ready(old));
        assert!(!state.accepts_ready(state.revision));
        state.replace(WorkspaceContent::Settings {
            tab: "shortcuts".into(),
        });
        assert!(!state.accepts_ready(old));
        assert!(state.accepts_ready(state.revision));
    }
}
