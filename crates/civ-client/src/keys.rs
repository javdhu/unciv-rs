//! Every key the client answers (docs/QUALITY-SPEC.md, K2: a small set of its own), in one table that the input
//! systems read and the key map lists, so the map cannot leave a key out.

use bevy::input::ButtonInput;
use bevy::prelude::KeyCode;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Act {
    PanUp,
    PanDown,
    PanLeft,
    PanRight,
    EndTurn,
    Close,
    Undo,
    Fortify,
    Wait,
    Found,
    NextUnit,
    PreviousUnit,
    Tree,
    Policies,
    Civilizations,
    Empire,
    Units,
    Log,
    Help,
    Grid,
    Timings,
    Pause,
    Speed1,
    Speed2,
    Speed3,
    Speed4,
    Step,
    SkipAnimations,
    Follow,
}

/// A modifier a key needs held.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mods {
    None,
    Shift,
    /// Ctrl, or Cmd on a Mac.
    Command,
}

pub struct Key {
    pub act: Act,
    pub context: &'static str,
    pub codes: &'static [KeyCode],
    pub mods: Mods,
    /// How the key map writes the keys.
    pub keys: &'static str,
    pub what: &'static str,
}

use Act::*;
use KeyCode as K;

pub const KEYS: &[Key] = &[
    Key { act: PanUp, context: "Map", codes: &[K::KeyW, K::ArrowUp], mods: Mods::None, keys: "W  ↑", what: "Pan north" },
    Key { act: PanDown, context: "Map", codes: &[K::KeyS, K::ArrowDown], mods: Mods::None, keys: "S  ↓", what: "Pan south" },
    Key { act: PanLeft, context: "Map", codes: &[K::KeyA, K::ArrowLeft], mods: Mods::None, keys: "A  ←", what: "Pan west" },
    Key { act: PanRight, context: "Map", codes: &[K::KeyD, K::ArrowRight], mods: Mods::None, keys: "D  →", what: "Pan east" },
    Key { act: Grid, context: "Map", codes: &[K::KeyG], mods: Mods::None, keys: "G", what: "Show or hide the hex grid" },
    Key { act: EndTurn, context: "Turn", codes: &[K::Enter, K::NumpadEnter], mods: Mods::None, keys: "Enter", what: "The decision End turn leads with, the next unit, or end the turn" },
    Key { act: Undo, context: "Turn", codes: &[K::KeyZ], mods: Mods::Command, keys: "Ctrl/Cmd Z", what: "Take back the last order" },
    Key { act: Close, context: "Turn", codes: &[K::Escape], mods: Mods::None, keys: "Esc", what: "Close the open panel, else clear the selection" },
    Key { act: NextUnit, context: "Units", codes: &[K::KeyN], mods: Mods::None, keys: "N", what: "Next unit that needs orders" },
    Key { act: PreviousUnit, context: "Units", codes: &[K::KeyN], mods: Mods::Shift, keys: "Shift N", what: "Previous unit that needs orders" },
    Key { act: Fortify, context: "Units", codes: &[K::KeyF], mods: Mods::None, keys: "F", what: "Fortify the selected unit" },
    Key { act: Wait, context: "Units", codes: &[K::Space], mods: Mods::None, keys: "Space", what: "The selected unit waits this turn" },
    Key { act: Found, context: "Units", codes: &[K::KeyB], mods: Mods::None, keys: "B", what: "Found a city with the selected settler" },
    Key { act: Tree, context: "Screens", codes: &[K::KeyT], mods: Mods::None, keys: "T", what: "Tech tree" },
    Key { act: Policies, context: "Screens", codes: &[K::KeyP], mods: Mods::None, keys: "P", what: "Policies" },
    Key { act: Civilizations, context: "Screens", codes: &[K::KeyC], mods: Mods::None, keys: "C", what: "Civilizations and diplomacy" },
    Key { act: Empire, context: "Screens", codes: &[K::KeyE], mods: Mods::None, keys: "E", what: "Your empire" },
    Key { act: Units, context: "Screens", codes: &[K::KeyU], mods: Mods::None, keys: "U", what: "Every unit" },
    Key { act: Log, context: "Screens", codes: &[K::KeyL], mods: Mods::None, keys: "L", what: "The log of what happened" },
    Key { act: Help, context: "Screens", codes: &[K::KeyH, K::Slash], mods: Mods::None, keys: "H  ?", what: "This key map" },
    Key { act: Timings, context: "Screens", codes: &[K::F3], mods: Mods::None, keys: "F3", what: "Frame and round timings" },
    Key { act: Pause, context: "Watching", codes: &[K::Space], mods: Mods::None, keys: "Space", what: "Pause or resume" },
    Key { act: Speed1, context: "Watching", codes: &[K::Digit1], mods: Mods::None, keys: "1", what: "Play at 1x" },
    Key { act: Speed2, context: "Watching", codes: &[K::Digit2], mods: Mods::None, keys: "2", what: "Play at 2x" },
    Key { act: Speed3, context: "Watching", codes: &[K::Digit3], mods: Mods::None, keys: "3", what: "Play at 4x" },
    Key { act: Speed4, context: "Watching", codes: &[K::Digit4], mods: Mods::None, keys: "4", what: "Play at 8x" },
    Key { act: Step, context: "Watching", codes: &[K::Period], mods: Mods::None, keys: ".", what: "Play one round" },
    Key { act: SkipAnimations, context: "Watching", codes: &[K::KeyK], mods: Mods::None, keys: "K", what: "Skip animations" },
    Key { act: Follow, context: "Watching", codes: &[K::KeyF], mods: Mods::None, keys: "F", what: "Follow the action" },
];

/// What the mouse does, for the key map.
pub const MOUSE: &[(&str, &str)] = &[
    ("Wheel  pinch", "Zoom toward the pointer"),
    ("Drag", "Pan"),
    ("Click a unit or city", "Select it"),
    ("Click a tile", "Move the selected unit there, or work it while a city is open"),
    ("Click an enemy", "Aim at it; click again to attack"),
    ("Hover", "What it is, and where its numbers come from"),
    ("Minimap", "Click or drag to look there"),
];

fn key(act: Act) -> &'static Key {
    KEYS.iter().find(|k| k.act == act).expect("every act has a key")
}

fn mods_held(keys: &ButtonInput<KeyCode>, mods: Mods) -> bool {
    let shift = keys.pressed(K::ShiftLeft) || keys.pressed(K::ShiftRight);
    let command = keys.pressed(K::ControlLeft) || keys.pressed(K::ControlRight) || keys.pressed(K::SuperLeft) || keys.pressed(K::SuperRight);
    match mods {
        Mods::None => !command && !shift,
        Mods::Shift => shift && !command,
        Mods::Command => command,
    }
}

/// Whether `act`'s key went down this frame, with its modifiers.
pub fn just(keys: &ButtonInput<KeyCode>, act: Act) -> bool {
    let k = key(act);
    // `?` is Shift with `/` on most layouts, so the key map's key takes Shift too.
    let mods = mods_held(keys, k.mods) || (act == Help && mods_held(keys, Mods::Shift));
    mods && k.codes.iter().any(|c| keys.just_pressed(*c))
}

/// Whether `act`'s key is held, with its modifiers.
pub fn held(keys: &ButtonInput<KeyCode>, act: Act) -> bool {
    let k = key(act);
    mods_held(keys, k.mods) && k.codes.iter().any(|c| keys.pressed(*c))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[Act] = &[
        PanUp, PanDown, PanLeft, PanRight, EndTurn, Close, Undo, Fortify, Wait, Found, NextUnit, PreviousUnit, Tree,
        Policies, Civilizations, Empire, Units, Log, Help, Grid, Timings, Pause, Speed1, Speed2, Speed3, Speed4, Step,
        SkipAnimations, Follow,
    ];

    /// Every act has exactly one row in the key map, and within a context no two rows share a key.
    #[test]
    fn the_key_map_lists_every_key_once_and_none_clash() {
        for act in ALL {
            assert_eq!(KEYS.iter().filter(|k| k.act == *act).count(), 1, "{act:?}");
        }
        assert_eq!(KEYS.len(), ALL.len());
        // Map and turn keys apply everywhere in play, with the unit and screen keys.
        let together = |a: &str, b: &str| a == b || (a != "Watching" && b != "Watching");
        for (i, a) in KEYS.iter().enumerate() {
            for b in &KEYS[i + 1..] {
                if together(a.context, b.context) && a.mods == b.mods {
                    assert!(!a.codes.iter().any(|c| b.codes.contains(c)), "{:?} and {:?} share a key", a.act, b.act);
                }
            }
        }
    }
}
