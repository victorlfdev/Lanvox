//! Shared avatar and presence-indicator components.
//!
//! Every surface (DM list, member list, friends, chat, user panel) draws avatars
//! and presence dots through here so they stay consistent: real image when the
//! user has one, otherwise a coloured circle with initials, plus a presence dot
//! with a distinct colour *and* shape per state.

use egui::{vec2, Align2, Color32, FontId, Rect, Response, Sense, Stroke, TextureHandle, Ui};

use super::theme;
use crate::app::LanvoxApp;
use crate::model::{color32, Presence, UserId, AVATAR_COLORS};
use crate::state::ServerView;

/// Standard avatar sizes (diameter in points).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Xs/Xl reserved for voice rows and profile cards
pub enum AvatarSize {
    /// Voice participant rows.
    Xs,
    /// Lists and the user panel.
    Sm,
    /// Chat messages and friend rows.
    Md,
    /// Call banners.
    Lg,
    /// Profile cards.
    Xl,
}

impl AvatarSize {
    pub fn px(self) -> f32 {
        match self {
            AvatarSize::Xs => 24.0,
            AvatarSize::Sm => 32.0,
            AvatarSize::Md => 40.0,
            AvatarSize::Lg => 44.0,
            AvatarSize::Xl => 80.0,
        }
    }
}

/// What the presence indicator should show, after combining connection state
/// with the user's chosen presence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresenceState {
    Online,
    Away,
    DoNotDisturb,
    /// Disconnected, or connected but set to invisible.
    Offline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresenceShape {
    /// Filled disc.
    Disc,
    /// Disc with a crescent bitten out.
    Crescent,
    /// Disc with a horizontal bar.
    Bar,
    /// Ring with a hollow centre.
    Ring,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresenceVisual {
    pub color: Color32,
    pub shape: PresenceShape,
}

impl PresenceState {
    /// `profile` is `None` when the user has no stored profile (treated as Online).
    pub fn resolve(connected: bool, profile: Option<Presence>) -> Self {
        if !connected {
            return PresenceState::Offline;
        }
        match profile.unwrap_or_default() {
            Presence::Online => PresenceState::Online,
            Presence::Away => PresenceState::Away,
            Presence::DoNotDisturb => PresenceState::DoNotDisturb,
            Presence::Invisible => PresenceState::Offline,
        }
    }

    /// Looks up connection state and stored profile for `user` in the current view.
    pub fn of_user(app: &LanvoxApp, user: UserId) -> Self {
        match app.view.as_ref() {
            Some(view) => Self::in_view(view, user),
            None => PresenceState::Offline,
        }
    }

    pub fn in_view(view: &ServerView, user: UserId) -> Self {
        let connected = view.users.get(&user).is_some_and(|u| u.online);
        let presence = view
            .workspace
            .profiles
            .iter()
            .find(|p| p.user == user)
            .map(|p| p.presence);
        Self::resolve(connected, presence)
    }

    /// Anyone shown as available in some form (used for grouping/sorting).
    pub fn is_present(self) -> bool {
        self != PresenceState::Offline
    }

    pub fn label(self) -> &'static str {
        match self {
            PresenceState::Online => "Disponível",
            PresenceState::Away => "Ausente",
            PresenceState::DoNotDisturb => "Não perturbar",
            PresenceState::Offline => "Offline",
        }
    }

    pub fn visual(self) -> PresenceVisual {
        match self {
            PresenceState::Online => PresenceVisual {
                color: theme::color(theme::GREEN),
                shape: PresenceShape::Disc,
            },
            PresenceState::Away => PresenceVisual {
                color: theme::color(theme::YELLOW),
                shape: PresenceShape::Crescent,
            },
            PresenceState::DoNotDisturb => PresenceVisual {
                color: theme::color(theme::RED),
                shape: PresenceShape::Bar,
            },
            PresenceState::Offline => PresenceVisual {
                color: theme::color(theme::TEXT_FAINT),
                shape: PresenceShape::Ring,
            },
        }
    }
}

/// Up to two uppercase initials: first letters of the first two words.
/// Falls back to "?" for empty names.
pub fn initials(name: &str) -> String {
    let s: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(|c| c.to_uppercase())
        .collect();
    if s.is_empty() {
        "?".to_owned()
    } else {
        s
    }
}

/// Stable fallback colour for a user, picked from the avatar palette by id.
pub fn stable_color(user: UserId) -> u32 {
    AVATAR_COLORS[(user as usize) % AVATAR_COLORS.len()]
}

/// The user's own colour if known, otherwise the stable per-user one.
pub fn fallback_color(user: Option<UserId>, color: Option<u32>) -> u32 {
    color
        .or_else(|| user.map(stable_color))
        .unwrap_or(AVATAR_COLORS[0])
}

pub struct Avatar<'a> {
    name: &'a str,
    user: Option<UserId>,
    color: Option<u32>,
    texture: Option<&'a TextureHandle>,
    size: AvatarSize,
    presence: Option<PresenceState>,
    speaking: bool,
    backdrop: Color32,
}

impl<'a> Avatar<'a> {
    pub fn new(name: &'a str, size: AvatarSize) -> Self {
        Self {
            name,
            user: None,
            color: None,
            texture: None,
            size,
            presence: None,
            speaking: false,
            backdrop: theme::color(theme::SIDEBAR),
        }
    }
    pub fn user(mut self, user: UserId) -> Self {
        self.user = Some(user);
        self
    }
    pub fn color(mut self, color: u32) -> Self {
        self.color = Some(color);
        self
    }
    pub fn texture(mut self, texture: Option<&'a TextureHandle>) -> Self {
        self.texture = texture;
        self
    }
    pub fn presence(mut self, presence: PresenceState) -> Self {
        self.presence = Some(presence);
        self
    }
    pub fn speaking(mut self, speaking: bool) -> Self {
        self.speaking = speaking;
        self
    }
    /// Colour behind the avatar, used for the presence dot's cut-out border.
    pub fn backdrop(mut self, backdrop: Color32) -> Self {
        self.backdrop = backdrop;
        self
    }

    /// Allocates space and draws; the response senses clicks.
    pub fn show(self, ui: &mut Ui) -> Response {
        let d = self.size.px();
        let (rect, resp) = ui.allocate_exact_size(vec2(d, d), Sense::click());
        let name = self.name.to_owned();
        self.paint(ui, rect);
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                ui.is_enabled(),
                format!("Perfil de {name}"),
            )
        });
        resp
    }

    /// Draws into an existing rect (for custom-laid-out rows).
    pub fn paint(self, ui: &Ui, rect: Rect) {
        let p = ui.painter();
        let c = rect.center();
        let radius = rect.width() / 2.0;
        if let Some(texture) = self.texture {
            egui::Image::new((texture.id(), rect.size()))
                .corner_radius(radius)
                .paint_at(ui, rect);
        } else {
            p.circle_filled(c, radius, color32(fallback_color(self.user, self.color)));
            p.text(
                c,
                Align2::CENTER_CENTER,
                initials(self.name),
                FontId::proportional(radius * 0.8),
                Color32::WHITE,
            );
        }
        if self.speaking {
            p.circle_stroke(
                c,
                radius + 1.5,
                Stroke::new(2.5, theme::color(theme::GREEN)),
            );
        }
        if let Some(state) = self.presence {
            let dot = c + vec2(radius * 0.72, radius * 0.72);
            paint_presence(ui, dot, (radius * 0.32).max(4.0), state, self.backdrop);
        }
    }
}

/// Draws a presence dot of radius `r` centred at `at`.
pub fn paint_presence(ui: &Ui, at: egui::Pos2, r: f32, state: PresenceState, backdrop: Color32) {
    let p = ui.painter();
    let v = state.visual();
    p.circle_filled(at, r + 2.0, backdrop);
    match v.shape {
        PresenceShape::Disc => {
            p.circle_filled(at, r, v.color);
        }
        PresenceShape::Crescent => {
            p.circle_filled(at, r, v.color);
            p.circle_filled(at + vec2(-r * 0.4, -r * 0.4), r * 0.62, backdrop);
        }
        PresenceShape::Bar => {
            p.circle_filled(at, r, v.color);
            p.rect_filled(
                Rect::from_center_size(at, vec2(r * 1.2, (r * 0.38).max(1.5))),
                1.0,
                backdrop,
            );
        }
        PresenceShape::Ring => {
            p.circle_filled(at, r, v.color);
            p.circle_filled(at, r * 0.45, backdrop);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_cases() {
        assert_eq!(initials("ana"), "A");
        assert_eq!(initials("ana maria souza"), "AM");
        assert_eq!(initials("  joão   "), "J");
        assert_eq!(initials("éva"), "É");
        assert_eq!(initials("   "), "?");
        assert_eq!(initials(""), "?");
        assert_eq!(initials("- x"), "X");
    }

    #[test]
    fn color_is_stable_and_in_palette() {
        for id in 0..40u32 {
            let c = stable_color(id as _);
            assert_eq!(c, stable_color(id as _));
            assert!(AVATAR_COLORS.contains(&c));
        }
        assert_ne!(stable_color(0), stable_color(1));
    }

    #[test]
    fn fallback_prefers_own_color() {
        assert_eq!(fallback_color(Some(3), Some(0x123456)), 0x123456);
        assert_eq!(fallback_color(Some(3), None), stable_color(3));
        assert_eq!(fallback_color(None, None), AVATAR_COLORS[0]);
    }

    #[test]
    fn presence_resolution() {
        use PresenceState::*;
        assert_eq!(
            PresenceState::resolve(false, Some(Presence::Online)),
            Offline
        );
        assert_eq!(PresenceState::resolve(true, None), Online);
        assert_eq!(PresenceState::resolve(true, Some(Presence::Away)), Away);
        assert_eq!(
            PresenceState::resolve(true, Some(Presence::DoNotDisturb)),
            DoNotDisturb
        );
        assert_eq!(
            PresenceState::resolve(true, Some(Presence::Invisible)),
            Offline
        );
        assert!(!Offline.is_present());
        assert!(Away.is_present());
    }

    #[test]
    fn presence_visuals_are_distinct() {
        use PresenceState::*;
        let all = [Online, Away, DoNotDisturb, Offline];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.visual().color, b.visual().color);
                assert_ne!(a.visual().shape, b.visual().shape);
            }
        }
        assert_eq!(Online.visual().color, theme::color(theme::GREEN));
        assert_eq!(Away.visual().color, theme::color(theme::YELLOW));
        assert_eq!(DoNotDisturb.visual().color, theme::color(theme::RED));
        assert_eq!(Away.label(), "Ausente");
    }
}
