//! The server rail on the far left and the channel list with the voice and user
//! panels at its bottom.

use std::sync::atomic::Ordering;

use egui::{
    pos2, vec2, Align, Align2, Color32, FontId, Frame, Layout, Margin, Panel, Rect, RichText,
    Sense, Stroke,
};

use super::avatar::{Avatar, AvatarSize, PresenceState};

use super::icons::{self, Icon};

use super::{theme, widgets};

use crate::app::{LanvoxApp, Modal, SettingsTab};

use crate::categories::{Row, RowKind, SidebarDrag};

use crate::model::*;

mod channel_row;
mod channels;
mod home_list;
mod rail;
mod voice_panel;
use channel_row::*;
pub use channels::*;
use home_list::*;
pub use rail::*;
use voice_panel::*;

/// Height of a category header row and of the thin "top of the list" drop strip.
const HEADER_ROW_H: f32 = 26.0;

const TOP_STRIP_H: f32 = 6.0;

struct HeaderResult {
    rect: Rect,
    toggle: bool,
}
