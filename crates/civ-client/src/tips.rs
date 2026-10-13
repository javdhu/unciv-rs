//! Tooltips (docs/QUALITY-SPEC.md, K1): any panel node with a `Tip` says what it is after the pointer rests on
//! it for 0.4 s. The card sits below what is hovered, or above it when there is no room, never over it, and
//! always inside the window.

use crate::inspect::UiFont;
use crate::playui::{PANEL, TEXT};
use bevy::picking::hover::HoverMap;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

/// What a control or number is, in a line or two.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct Tip(pub String);

impl Tip {
    pub fn new(text: impl Into<String>) -> Tip {
        Tip(text.into())
    }
}

/// A tip to show as if its node were hovered, for review recordings that have no pointer.
#[derive(Resource, Default)]
pub struct Forced(pub Option<Entity>);

#[derive(Component)]
struct Card;
#[derive(Component)]
struct CardText;

const DELAY: f32 = 0.4;
const GAP: f32 = 6.0;

pub fn plugin(app: &mut App) {
    app.init_resource::<Forced>().add_systems(Startup, card).add_systems(Update, show);
}

fn card(mut commands: Commands, font: Res<UiFont>) {
    commands
        .spawn((
            Card,
            crate::app::Persist,
            Node {
                position_type: PositionType::Absolute,
                max_width: Val::Px(320.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(5.0)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(PANEL.with_alpha(0.96)),
            Outline::new(Val::Px(1.0), Val::ZERO, Color::srgba(1.0, 1.0, 1.0, 0.18)),
            GlobalZIndex(60),
        ))
        .with_child((
            CardText,
            Text::default(),
            TextFont { font: font.regular.clone().into(), font_size: FontSize::Px(12.0), ..default() },
            TextColor(TEXT),
        ));
}

/// Where a card of `card` size goes for a hovered box `target`, in a window of `window` size (all logical
/// pixels): below the target, else above it, its left edge at the target's, kept inside the window.
pub fn place(card: Vec2, target: Rect, window: Vec2) -> Vec2 {
    let x = target.min.x.min(window.x - card.x - GAP).max(GAP);
    let below = target.max.y + GAP;
    let y = if below + card.y <= window.y - GAP { below } else { (target.min.y - GAP - card.y).max(GAP) };
    Vec2::new(x, y)
}

#[allow(clippy::too_many_arguments)]
fn show(
    hover_map: Res<HoverMap>,
    tips: Query<(&Tip, &ComputedNode, &UiGlobalTransform)>,
    parents: Query<&ChildOf>,
    windows: Query<&Window, With<PrimaryWindow>>,
    ui_scale: Res<UiScale>,
    time: Res<Time>,
    forced: Res<Forced>,
    mut rest: Local<(Option<Entity>, f32)>,
    mut cards: Query<(&mut Node, &ComputedNode), (With<Card>, Without<Tip>)>,
    mut texts: Query<&mut Text, With<CardText>>,
) {
    let (Ok((mut node, card)), Ok(mut text), Ok(window)) = (cards.single_mut(), texts.single_mut(), windows.single()) else { return };
    // The tipped node under the pointer: the nearest ancestor of anything hovered that has a tip.
    let hovered = hover_map
        .values()
        .flat_map(|m| m.keys())
        .copied()
        .find_map(|e| std::iter::once(e).chain(parents.iter_ancestors(e)).find(|a| tips.contains(*a)))
        .or(forced.0.filter(|e| tips.contains(*e)));
    if rest.0 != hovered {
        *rest = (hovered, 0.0);
    } else {
        rest.1 += time.delta_secs();
    }
    let Some((tip, target, at)) = hovered.filter(|_| rest.1 >= DELAY).and_then(|e| tips.get(e).ok()) else {
        node.display = Display::None;
        return;
    };
    if text.0 != tip.0 {
        text.0 = tip.0.clone();
    }
    node.display = Display::Flex;
    // Physical pixels to the panels' own units (logical pixels over the UI scale).
    let to_ui = 1.0 / (window.scale_factor() * ui_scale.0);
    let size = target.size() * to_ui;
    let centre = at.translation * to_ui;
    let rect = Rect::from_center_size(centre, size);
    let window = Vec2::new(window.width(), window.height()) / ui_scale.0;
    let at = place(card.size() * to_ui, rect, window);
    node.left = Val::Px(at.x);
    node.top = Val::Px(at.y);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_never_cover_what_is_hovered_and_stay_inside_the_window() {
        let window = Vec2::new(1600.0, 1000.0);
        let card = Vec2::new(300.0, 60.0);
        for target in [
            Rect::new(10.0, 10.0, 80.0, 40.0),
            Rect::new(1500.0, 10.0, 1590.0, 40.0),
            Rect::new(10.0, 950.0, 300.0, 990.0),
            Rect::new(1360.0, 930.0, 1590.0, 990.0),
            Rect::new(700.0, 480.0, 900.0, 520.0),
        ] {
            let at = place(card, target, window);
            let placed = Rect::from_corners(at, at + card);
            assert!(placed.intersect(target).is_empty(), "{target:?} covered by {placed:?}");
            assert!(placed.min.x >= 0.0 && placed.min.y >= 0.0 && placed.max.x <= window.x && placed.max.y <= window.y, "{placed:?} outside");
        }
    }
}

/// A civ-wide stat's sources for its tooltip, largest first, once the view carries them.
pub fn breakdown_lines(_v: &civ_sim::PlayerView, _stat: &str) -> Vec<String> {
    Vec::new()
}
