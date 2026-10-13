//! Settings (docs/QUALITY-SPEC.md): the UI scale (K5: automatic from the window, with an adjustment), panning
//! at the window's edge, and camera speed. Kept per player: `<config>/settings.json` natively, localStorage in
//! the browser. Opened from Home and from the launch bar.

use crate::inspect::UiFont;
use crate::playui::{clear, text, BUTTON, BUTTON_ON, DIM, TEXT};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{observe, Activate, Button};
use bevy::window::PrimaryWindow;

#[derive(Resource, Clone, Debug, PartialEq)]
pub struct Settings {
    /// Multiplies the automatic scale: 0.75 to 1.5.
    pub ui_scale: f32,
    pub edge_pan: bool,
    /// Multiplies keyboard and edge panning: 0.5 to 2.
    pub camera_speed: f32,
    pub open: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { ui_scale: 1.0, edge_pan: false, camera_speed: 1.0, open: false }
    }
}

impl Settings {
    fn to_json(&self) -> String {
        serde_json::json!({ "uiScale": self.ui_scale, "edgePan": self.edge_pan, "cameraSpeed": self.camera_speed }).to_string()
    }

    fn from_json(text: &str) -> Option<Settings> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        Some(Settings {
            ui_scale: (v.get("uiScale")?.as_f64()? as f32).clamp(0.75, 1.5),
            edge_pan: v.get("edgePan")?.as_bool()?,
            camera_speed: (v.get("cameraSpeed")?.as_f64()? as f32).clamp(0.5, 2.0),
            open: false,
        })
    }
}

/// The scale a window's panels get on their own: they were laid out for 1600 by 1000, and grow or shrink with
/// the window, within reason.
pub fn automatic(width: f32, height: f32) -> f32 {
    (width / 1600.0).min(height / 1000.0).clamp(0.8, 1.6)
}

#[derive(Component)]
struct Panel;

#[derive(Component, Clone, Copy, PartialEq)]
enum Press {
    Smaller,
    Larger,
    EdgePan,
    Slower,
    Faster,
    Close,
}

pub fn plugin(app: &mut App) {
    app.insert_resource(load())
        .add_systems(Startup, panel)
        .add_systems(Update, (scale, save, draw, shade).chain());
}

fn panel(mut commands: Commands) {
    commands.spawn((
        Panel,
        crate::app::Persist,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Percent(20.0),
            left: Val::Percent(50.0),
            width: Val::Px(360.0),
            padding: UiRect::all(Val::Px(14.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(8.0),
            ..default()
        },
        UiTransform { translation: Val2::percent(-50.0, 0.0), ..default() },
        BackgroundColor(crate::screens::SOLID_PANEL),
        GlobalZIndex(45),
        Visibility::Hidden,
    ));
}

fn scale(settings: Res<Settings>, windows: Query<&Window, With<PrimaryWindow>>, mut ui: ResMut<UiScale>, shots: Option<Res<crate::shots::Shots>>) {
    // Review shots keep one size everywhere.
    if shots.is_some() {
        return;
    }
    let Ok(window) = windows.single() else { return };
    let want = automatic(window.width(), window.height()) * settings.ui_scale;
    if (ui.0 - want).abs() > 0.001 {
        ui.0 = want;
    }
}

fn save(settings: Res<Settings>, mut last: Local<Option<String>>) {
    let json = settings.to_json();
    if last.as_deref() == Some(json.as_str()) {
        return;
    }
    if last.is_some() {
        store(&json);
    }
    *last = Some(json);
}

fn draw(mut commands: Commands, settings: Res<Settings>, font: Res<UiFont>, mut panel: Query<(Entity, &mut Visibility, Option<&Children>), With<Panel>>) {
    let Ok((e, mut visibility, children)) = panel.single_mut() else { return };
    if !settings.is_changed() {
        return;
    }
    clear(&mut commands, children);
    if !settings.open {
        *visibility = Visibility::Hidden;
        return;
    }
    *visibility = Visibility::Inherited;
    commands.entity(e).with_children(|p| {
        p.spawn(text(&font, "Settings", 18.0, TEXT));
        p.spawn(row()).with_children(|r| {
            r.spawn(text(&font, format!("Interface size {}%", (settings.ui_scale * 100.0).round()), 14.0, TEXT));
            r.spawn(button(&font, Press::Smaller, "−"));
            r.spawn(button(&font, Press::Larger, "+"));
        });
        p.spawn(text(&font, "Panels also grow and shrink with the window.", 12.0, DIM));
        p.spawn(row()).with_children(|r| {
            r.spawn(text(&font, format!("Camera speed {}%", (settings.camera_speed * 100.0).round()), 14.0, TEXT));
            r.spawn(button(&font, Press::Slower, "−"));
            r.spawn(button(&font, Press::Faster, "+"));
        });
        p.spawn(button(&font, Press::EdgePan, if settings.edge_pan { "Panning at the window's edge: on" } else { "Panning at the window's edge: off" }));
        p.spawn(button(&font, Press::Close, "Close"));
    });
}

fn row() -> Node {
    Node { column_gap: Val::Px(8.0), align_items: AlignItems::Center, ..default() }
}

fn button(font: &UiFont, press: Press, label: &str) -> impl Bundle {
    (
        press,
        Button,
        Hovered::default(),
        observe(on_press),
        Node { padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)), border_radius: BorderRadius::all(Val::Px(4.0)), ..default() },
        BackgroundColor(BUTTON),
        children![text(font, label, 13.0, TEXT)],
    )
}

fn on_press(event: On<Activate>, presses: Query<&Press>, mut settings: ResMut<Settings>) {
    let Ok(press) = presses.get(event.entity) else { return };
    // Whole steps of 5%, so repeated presses land on round numbers.
    let step = |v: f32, by: f32| ((v + by) * 20.0).round() / 20.0;
    match press {
        Press::Smaller => settings.ui_scale = step(settings.ui_scale, -0.05).clamp(0.75, 1.5),
        Press::Larger => settings.ui_scale = step(settings.ui_scale, 0.05).clamp(0.75, 1.5),
        Press::Slower => settings.camera_speed = step(settings.camera_speed, -0.25).clamp(0.5, 2.0),
        Press::Faster => settings.camera_speed = step(settings.camera_speed, 0.25).clamp(0.5, 2.0),
        Press::EdgePan => settings.edge_pan = !settings.edge_pan,
        Press::Close => settings.open = false,
    }
}

fn shade(mut buttons: Query<(&Hovered, &mut BackgroundColor), (With<Press>, Changed<Hovered>)>) {
    for (hovered, mut colour) in &mut buttons {
        colour.0 = if hovered.get() { BUTTON_ON } else { BUTTON };
    }
}

fn load() -> Settings {
    #[cfg(not(target_arch = "wasm32"))]
    let text = std::fs::read_to_string(crate::net::config_dir().join("settings.json")).ok();
    #[cfg(target_arch = "wasm32")]
    let text = web_sys::window().and_then(|w| w.local_storage().ok().flatten()).and_then(|s| s.get_item("civ.settings").ok().flatten());
    text.and_then(|t| Settings::from_json(&t)).unwrap_or_default()
}

fn store(json: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let dir = crate::net::config_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("settings.json"), json);
    }
    #[cfg(target_arch = "wasm32")]
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item("civ.settings", json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_read_back_and_scale_follows_the_window() {
        let s = Settings { ui_scale: 1.25, edge_pan: true, camera_speed: 1.5, open: false };
        assert_eq!(Settings::from_json(&s.to_json()), Some(s));
        assert_eq!(automatic(1600.0, 1000.0), 1.0);
        assert_eq!(automatic(1280.0, 800.0), 0.8);
        assert_eq!(automatic(3200.0, 1000.0), 1.0, "the shorter side decides");
        assert_eq!(automatic(800.0, 500.0), 0.8, "never below 80%");
        assert_eq!(automatic(5120.0, 2880.0), 1.6, "never above 160%");
    }
}
