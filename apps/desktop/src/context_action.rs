use paste_domain::PinboardId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", content = "pinboard_id", rename_all = "snake_case")]
pub enum ContextAction {
    Paste,
    PastePlain,
    Copy,
    CopyPlain,
    Preview,
    Edit,
    Rename,
    Delete,
    ToggleStack,
    Locate,
    Pin(PinboardId),
    Unpin(PinboardId),
}
