//! Presentation (docs/VISUAL-SPEC.md, Motion): a round's events played as timelines over the pieces.
//!
//! Views establish state, events animate it (docs/CLIENT-PROTOCOL.md). A round's events split into beats, one
//! per acting civ in order; a beat walks its units at once (0.22 s a step), then plays its battles one by one
//! (a melee lunge or a projectile, the defender's flinch, the damage rising; the fallen sink), and new or taken
//! cities rise. At 1x a round is capped at 8 s, every duration scaled to fit but no step under 0.05 s; 2x, 4x
//! and 8x divide time; skipping snaps to the view. The next round waits for the last beat and 0.5 s more.
//!
//! While a unit walks, presentation holds it (`Held`) where it is in the story; a unit the view no longer has
//! but whose fall is still to be played is kept (`Doomed`) until it falls.

use crate::camera::MapCamera;
use crate::client::{Gate, RoundEvents};
use crate::hex;
use crate::inspect::UiFont;
use crate::map::Land;
use crate::pieces::{Held, Pieces, UnitPiece};
use bevy::prelude::*;
use civ_sim::protocol::{AttackTarget, PresentedEvent};
use civ_sim::PresentationEvent as E;
use std::collections::{HashMap, HashSet};

/// (id, generation) of either of civ-sim's entity references (the protocol's and the view's).
macro_rules! key {
    ($r:expr) => {
        ($r.id, $r.r#gen)
    };
}

const STEP: f32 = 0.22;
const TURN: f32 = 0.10;
const LUNGE: f32 = 0.18;
const FLINCH: f32 = 0.10;
const RISE_NUMBER: f32 = 0.7;
const PROJECTILE: f32 = 0.35;
const FALL: f32 = 0.45;
const FOUND: f32 = 0.6;
const ROUND_CAP: f32 = 8.0;
const MIN_STEP: f32 = 0.05;
const HOLD: f32 = 0.5;
const GLIDE: f32 = 0.6;
/// Manual camera input suspends following for this long.
const FOLLOW_PAUSE: f64 = 3.0;

/// What the viewer asked of playback; the HUD and the keyboard set it.
#[derive(Resource, Debug, Clone, Copy)]
pub struct Playback {
    pub speed: f32,
    pub paused: bool,
    /// One more round while paused.
    pub step: bool,
    pub skip: bool,
    pub follow: bool,
}

impl Default for Playback {
    fn default() -> Self {
        Playback { speed: 1.0, paused: false, step: false, skip: false, follow: true }
    }
}

/// A hook for sound (docs/VISUAL-SPEC.md, V3): written as each moment plays; nothing consumes it yet.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    Round,
    Move,
    Attack,
    Fall,
    Founding,
    Capture,
}

#[derive(Component)]
pub struct Doomed;

#[derive(Component)]
struct Rising;

#[derive(Component)]
struct Number {
    at: Vec3,
    age: f32,
}

/// The one projectile shape, made once.
#[derive(Resource)]
struct Shot(Handle<Mesh>, Handle<StandardMaterial>);

#[derive(Component)]
struct Projectile {
    from: Vec3,
    to: Vec3,
    age: f32,
}

struct Walk {
    unit: Entity,
    path: Vec<u16>,
}

struct Strike {
    attacker: Option<Entity>,
    defender: Option<Entity>,
    from: u16,
    at: u16,
    ranged: bool,
    damage: i32,
    fallen: Vec<Entity>,
}

#[derive(Default)]
struct Beat {
    walks: Vec<Walk>,
    strikes: Vec<Strike>,
    rises: Vec<Entity>,
    cues: Vec<Cue>,
}

impl Beat {
    fn walking(&self) -> f32 {
        self.walks.iter().map(|w| w.path.len().saturating_sub(1) as f32 * STEP).fold(0.0, f32::max)
            .max(if self.rises.is_empty() { 0.0 } else { FOUND })
    }

    fn striking(&self) -> f32 {
        self.strikes.iter().map(strike_length).sum()
    }

    fn length(&self) -> f32 {
        self.walking() + self.striking()
    }

    fn focus(&self, land: &Land) -> Option<Vec2> {
        let tile = self.walks.first().and_then(|w| w.path.last().copied()).or_else(|| self.strikes.first().map(|s| s.at));
        tile.map(|t| land.0.grid.centre(t))
    }
}

fn strike_length(s: &Strike) -> f32 {
    let blow = if s.ranged { PROJECTILE + FLINCH } else { 2.0 * LUNGE };
    blow + if s.fallen.is_empty() { 0.0 } else { FALL }
}

#[derive(Resource, Default)]
struct Show {
    beats: Vec<Beat>,
    /// Time into the current beat, in story seconds.
    clock: f32,
    /// Story seconds per real second at 1x: above 1 when a long round is squeezed into the cap.
    squeeze: f32,
    /// Real seconds since the last beat ended.
    idle: f32,
    /// Camera glide toward the current beat: start, end, progress.
    glide: Option<(Vec2, Vec2, f32)>,
    started: bool,
    seen: HashSet<String>,
}

impl Show {
    fn playing(&self) -> bool {
        !self.beats.is_empty()
    }
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Playback>()
        .init_resource::<Show>()
        .insert_resource(Gate { open: true })
        .add_message::<Cue>()
        .add_systems(Startup, |mut c: Commands, mut m: ResMut<Assets<Mesh>>, mut s: ResMut<Assets<StandardMaterial>>| {
            c.insert_resource(Shot(
                m.add(Sphere::new(0.04).mesh().ico(1).expect("tiny ico")),
                s.add(StandardMaterial { base_color: Color::srgb(0.2, 0.18, 0.15), ..default() }),
            ));
        })
        .add_systems(
            Update,
            (
                doom.before(crate::pieces::PiecesSet::Project),
                begin.after(crate::pieces::PiecesSet::Project).before(crate::pieces::PiecesSet::Stand),
                (keys, play, numbers, projectiles, gate).chain().after(begin).before(crate::pieces::PiecesSet::Stand),
            )
                .in_set(crate::app::InMatch),
        )
        .add_systems(
            OnExit(crate::app::AppState::Match),
            (crate::app::reset::<Playback>, crate::app::reset::<Show>, |mut c: Commands| c.insert_resource(Gate { open: true })),
        );
}

/// Watching's keys (`keys.rs`); in play, Space and F belong to units.
fn keys(keys: Res<ButtonInput<KeyCode>>, mut playback: ResMut<Playback>, mode: Option<Res<crate::play::Mode>>) {
    use crate::keys::{just, Act};
    if mode.is_some_and(|m| *m == crate::play::Mode::Play) {
        return;
    }
    if just(&keys, Act::Pause) {
        playback.paused = !playback.paused;
    }
    for (act, speed) in [(Act::Speed1, 1.0), (Act::Speed2, 2.0), (Act::Speed3, 4.0), (Act::Speed4, 8.0)] {
        if just(&keys, act) {
            playback.speed = speed;
        }
    }
    if just(&keys, Act::Step) {
        playback.step = true;
    }
    if just(&keys, Act::SkipAnimations) {
        playback.skip = !playback.skip;
    }
    if just(&keys, Act::Follow) {
        playback.follow = !playback.follow;
    }
}

fn fallen(e: &E) -> Vec<(u32, u32)> {
    match e {
        E::Attacked { attacker, target, attacker_died, defender_died, .. } => {
            let mut out = Vec::new();
            if let (true, AttackTarget::Unit { unit }) = (*attacker_died, attacker) {
                out.push(key!(unit));
            }
            if let (true, AttackTarget::Unit { unit }) = (*defender_died, target) {
                out.push(key!(unit));
            }
            out
        }
        E::UnitDamaged { unit, died: true, .. } => vec![key!(unit)],
        _ => Vec::new(),
    }
}

/// Keep the pieces whose fall this round will play, before the projection drops them.
fn doom(mut commands: Commands, mut rounds: MessageReader<RoundEvents>, pieces: Res<Pieces>, playback: Res<Playback>) {
    for RoundEvents(events) in rounds.read() {
        if playback.skip {
            continue;
        }
        for event in events {
            for k in fallen(&event.event.event) {
                if let Some(e) = pieces.units.get(&k) {
                    commands.entity(*e).insert(Doomed);
                }
            }
        }
    }
}

/// Build the beats of a new round and hold every walker where its walk starts.
#[allow(clippy::too_many_arguments)]
fn begin(
    mut commands: Commands,
    mut rounds: MessageReader<RoundEvents>,
    mut show: ResMut<Show>,
    mut cues: MessageWriter<Cue>,
    pieces: Res<Pieces>,
    land: Option<Res<Land>>,
    playback: Res<Playback>,
    units: Query<&UnitPiece>,
    mut cities: Query<&mut Transform, Without<UnitPiece>>,
) {
    let Some(land) = land else {
        rounds.clear();
        return;
    };
    for RoundEvents(events) in rounds.read() {
        let fresh: Vec<&PresentedEvent> = events.iter().filter(|e| show.seen.insert(e.id.clone())).collect();
        if fresh.is_empty() {
            continue;
        }
        cues.write(Cue::Round);
        if playback.skip {
            continue;
        }
        let beats = beats(&fresh, &pieces, &units);
        for beat in &beats {
            for walk in &beat.walks {
                if let Some(&start) = walk.path.first() {
                    let g = land.0.grid.centre(start);
                    commands.entity(walk.unit).insert(Held(hex::world(g, surface(&land, g))));
                }
            }
            for city in &beat.rises {
                if let Ok(mut t) = cities.get_mut(*city) {
                    t.scale = Vec3::new(1.0, 0.0, 1.0);
                }
                commands.entity(*city).insert(Rising);
            }
        }
        let total: f32 = beats.iter().map(Beat::length).sum();
        show.squeeze = if total > ROUND_CAP { (total / ROUND_CAP).min(STEP / MIN_STEP) } else { 1.0 };
        show.beats.extend(beats);
        show.clock = 0.0;
        show.started = false;
    }
}

fn surface(land: &Land, g: Vec2) -> f32 {
    land.0.height(g).unwrap_or(0.0).max(0.0)
}

/// Group events by acting civ, in order.
fn beats(events: &[&PresentedEvent], pieces: &Pieces, units: &Query<&UnitPiece>) -> Vec<Beat> {
    let owner = |k: (u32, u32)| pieces.units.get(&k).and_then(|e| units.get(*e).ok()).map(|u| u.owner);
    let entity = |k: (u32, u32)| pieces.units.get(&k).copied();
    let mut out: Vec<(Option<u8>, Beat)> = Vec::new();
    let mut paths: HashMap<Entity, usize> = HashMap::new();
    for event in events {
        let e = &event.event.event;
        let actor = match e {
            E::UnitMoved { unit, .. } => owner(key!(unit)),
            E::Attacked { attacker: AttackTarget::Unit { unit }, .. } => owner(key!(unit)),
            E::CityFounded { by, .. } | E::CityCaptured { by, .. } => Some(*by),
            _ => None,
        };
        let Some(actor) = actor else {
            continue;
        };
        if out.last().is_none_or(|(a, _)| *a != Some(actor)) {
            out.push((Some(actor), Beat::default()));
            paths.clear();
        }
        let beat = &mut out.last_mut().expect("pushed above").1;
        match e {
            E::UnitMoved { unit, from, to, .. } => {
                let Some(u) = entity(key!(unit)) else { continue };
                match paths.get(&u) {
                    Some(&i) => beat.walks[i].path.push(*to),
                    None => {
                        paths.insert(u, beat.walks.len());
                        beat.walks.push(Walk { unit: u, path: vec![*from, *to] });
                    }
                }
                beat.cues.push(Cue::Move);
            }
            E::Attacked { attacker, target, from, at, ranged, damage_to_defender, .. } => {
                let unit_of = |t: &AttackTarget| match t {
                    AttackTarget::Unit { unit } => entity(key!(unit)),
                    AttackTarget::City { .. } => None,
                };
                let fallen: Vec<Entity> = fallen(e).into_iter().filter_map(entity).collect();
                if !fallen.is_empty() {
                    beat.cues.push(Cue::Fall);
                }
                beat.strikes.push(Strike {
                    attacker: unit_of(attacker),
                    defender: unit_of(target),
                    from: *from,
                    at: *at,
                    ranged: *ranged,
                    damage: *damage_to_defender,
                    fallen,
                });
                beat.cues.push(Cue::Attack);
            }
            E::CityFounded { city, .. } | E::CityCaptured { city, .. } => {
                if let Some(c) = pieces.cities.get(&key!(city)) {
                    beat.rises.push(*c);
                }
                beat.cues.push(if matches!(e, E::CityFounded { .. }) { Cue::Founding } else { Cue::Capture });
            }
            _ => {}
        }
    }
    out.into_iter().map(|(_, b)| b).filter(|b| b.length() > 0.0).collect()
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Advance the current beat; finish it, or everything when skipping.
#[allow(clippy::too_many_arguments)]
fn play(
    mut commands: Commands,
    time: Res<Time<Real>>,
    playback: Res<Playback>,
    mut show: ResMut<Show>,
    land: Option<Res<Land>>,
    font: Res<UiFont>,
    mut cues: MessageWriter<Cue>,
    mut units: Query<(&mut UnitPiece, &mut Transform, Option<&mut Held>, Has<Doomed>)>,
    mut cities: Query<&mut Transform, (Without<UnitPiece>, With<Rising>)>,
    mut cameras: Query<(&Camera, &GlobalTransform, &mut MapCamera)>,
    shot: Res<Shot>,
) {
    let Some(land) = land else { return };
    if playback.skip && show.playing() {
        for beat in std::mem::take(&mut show.beats) {
            finish(&mut commands, &beat, &mut units, &mut cities);
        }
        return;
    }
    if !show.playing() {
        show.idle += time.delta_secs();
        return;
    }
    show.idle = 0.0;
    if playback.paused {
        return;
    }
    let dt = time.delta_secs() * playback.speed * show.squeeze;
    // Follow the action: glide to an off-screen beat before playing it.
    let focus = show.beats[0].focus(&land);
    if !show.started {
        show.started = true;
        if let (true, Some(focus), Ok((camera, at, map))) = (playback.follow, focus, cameras.single()) {
            let recent = time.elapsed_secs_f64() - map.manual_at < FOLLOW_PAUSE;
            let size = camera.logical_viewport_size().unwrap_or(Vec2::ONE);
            let on_screen = camera
                .world_to_viewport(at, hex::world(focus, 0.0))
                .is_ok_and(|s| s.x > size.x * 0.1 && s.x < size.x * 0.9 && s.y > size.y * 0.1 && s.y < size.y * 0.9);
            if !recent && !on_screen {
                show.glide = Some((map.target, focus, 0.0));
            }
        }
        for cue in std::mem::take(&mut show.beats[0].cues) {
            cues.write(cue);
        }
    }
    if let Some((from, to, t)) = show.glide {
        let t = t + time.delta_secs() * playback.speed / GLIDE;
        if let Ok((_, _, mut map)) = cameras.single_mut() {
            map.target = from.lerp(to, ease(t));
        }
        show.glide = (t < 1.0).then_some((from, to, t));
        return;
    }
    show.clock += dt;
    let clock = show.clock;
    let beat = &show.beats[0];
    let walking = beat.walking();
    // Walks: everyone at once, a step every STEP seconds.
    for walk in &beat.walks {
        let Ok((mut piece, _, held, _)) = units.get_mut(walk.unit) else { continue };
        let steps = walk.path.len().saturating_sub(1).max(1) as f32;
        let t = (clock / STEP).min(steps);
        let i = (t.floor() as usize).min(walk.path.len() - 2);
        let (a, b) = (land.0.grid.centre(walk.path[i]), land.0.grid.centre(walk.path[i + 1]));
        let toward = b - a;
        let g = a.lerp(b, ease(t - i as f32));
        // Turn to face the step over TURN seconds.
        let want = (toward.y).atan2(toward.x);
        let blend = ((clock - i as f32 * STEP) / TURN).clamp(0.0, 1.0);
        piece.heading += angle_between(piece.heading, want) * blend;
        if let Some(mut h) = held {
            h.0 = hex::world(g, surface(&land, g));
        }
    }
    for city in &beat.rises {
        if let Ok(mut t) = cities.get_mut(*city) {
            t.scale.y = ease(clock / FOUND);
        }
    }
    if clock >= walking {
        // The walkers stand on their view tiles now.
        for walk in &beat.walks {
            commands.entity(walk.unit).remove::<Held>();
        }
        // Battles, one at a time.
        let mut start = walking;
        for strike in &beat.strikes {
            let length = strike_length(strike);
            if clock < start + length {
                let local = clock - start;
                act(&mut commands, strike, local, &land, &font, &mut units, &shot);
                break;
            }
            start += length;
        }
    }
    if clock >= beat.length() {
        let beat = show.beats.remove(0);
        finish(&mut commands, &beat, &mut units, &mut cities);
        show.clock = 0.0;
        show.started = false;
    }
}

fn angle_between(from: f32, to: f32) -> f32 {
    let d = (to - from).rem_euclid(std::f32::consts::TAU);
    if d > std::f32::consts::PI { d - std::f32::consts::TAU } else { d }
}

/// One moment of a battle, `t` seconds into it.
#[allow(clippy::too_many_arguments)]
fn act(
    commands: &mut Commands,
    s: &Strike,
    t: f32,
    land: &Land,
    font: &UiFont,
    units: &mut Query<(&mut UnitPiece, &mut Transform, Option<&mut Held>, Has<Doomed>)>,
    shot: &Shot,
) {
    let from = land.0.grid.centre(s.from);
    let at = land.0.grid.centre(s.at);
    let target = hex::world(at, surface(land, at) + 0.2);
    let blow = if s.ranged { PROJECTILE } else { LUNGE };
    // The blow: a lunge forward and back, or a projectile on its way.
    if !s.ranged {
        if let Some(a) = s.attacker {
            let toward = (at - from).normalize_or_zero();
            let reach = if t < LUNGE { ease(t / LUNGE) } else { 1.0 - ease((t - LUNGE) / LUNGE) };
            let g = from + toward * 0.30 * reach;
            hold(commands, units, a, hex::world(g, surface(land, g)));
        }
    } else if t < 1e-3 + 0.02 {
        commands.spawn((
            Projectile { from: hex::world(from, surface(land, from) + 0.3), to: target, age: 0.0 },
            Mesh3d(shot.0.clone()),
            MeshMaterial3d(shot.1.clone()),
            Transform::from_translation(hex::world(from, 0.3)),
        ));
    }
    // The defender flinches as the blow lands; the damage rises from it.
    if (blow..blow + FLINCH).contains(&t) {
        if let Some(Ok((_, mut transform, _, _))) = s.defender.map(|d| units.get_mut(d)) {
            transform.scale = Vec3::splat(1.15);
        }
    } else if let Some(Ok((_, mut transform, _, false))) = s.defender.map(|d| units.get_mut(d)) {
        transform.scale = Vec3::ONE;
    }
    if t >= blow && t < blow + 0.02 && s.damage > 0 {
        commands.spawn((
            Number { at: target, age: 0.0 },
            Text::new(format!("-{}", s.damage)),
            TextFont { font: font.semibold.clone().into(), font_size: FontSize::Px(15.0), ..default() },
            TextColor(Color::srgb(1.0, 0.86, 0.8)),
            Node { position_type: PositionType::Absolute, ..default() },
            Visibility::Hidden,
        ));
    }
    // The fallen sink and shrink.
    let falling = t - (blow + if s.ranged { FLINCH } else { LUNGE });
    if falling > 0.0 {
        for e in &s.fallen {
            let Ok((piece, mut transform, _, _)) = units.get_mut(*e) else { continue };
            let k = 1.0 - ease(falling / FALL);
            transform.scale = Vec3::splat(k.max(0.01));
            let g = land.0.grid.centre(piece.tile) + piece.offset;
            let p = hex::world(g, surface(land, g) - 0.15 * (1.0 - k));
            hold(commands, units, *e, p);
        }
    }
}

fn hold(
    commands: &mut Commands,
    units: &mut Query<(&mut UnitPiece, &mut Transform, Option<&mut Held>, Has<Doomed>)>,
    unit: Entity,
    at: Vec3,
) {
    match units.get_mut(unit) {
        Ok((_, _, Some(mut held), _)) => held.0 = at,
        Ok(_) => {
            commands.entity(unit).insert(Held(at));
        }
        Err(_) => {}
    }
}

fn finish(
    commands: &mut Commands,
    beat: &Beat,
    units: &mut Query<(&mut UnitPiece, &mut Transform, Option<&mut Held>, Has<Doomed>)>,
    cities: &mut Query<&mut Transform, (Without<UnitPiece>, With<Rising>)>,
) {
    for walk in &beat.walks {
        commands.entity(walk.unit).remove::<Held>();
    }
    for strike in &beat.strikes {
        for e in strike.attacker.iter().chain(&strike.defender) {
            if let Ok((_, mut t, _, _)) = units.get_mut(*e) {
                t.scale = Vec3::ONE;
            }
            commands.entity(*e).remove::<Held>();
        }
        for e in &strike.fallen {
            if let Ok((_, _, _, true)) = units.get(*e) {
                commands.entity(*e).despawn();
            }
        }
    }
    for city in &beat.rises {
        if let Ok(mut t) = cities.get_mut(*city) {
            t.scale = Vec3::ONE;
        }
        commands.entity(*city).remove::<Rising>();
    }
}

fn numbers(
    mut commands: Commands,
    time: Res<Time<Real>>,
    playback: Res<Playback>,
    cameras: Query<(&Camera, &GlobalTransform), With<MapCamera>>,
    mut numbers: Query<(Entity, &mut Number, &mut Node, &mut Visibility, &mut TextColor)>,
) {
    let Ok((camera, at)) = cameras.single() else { return };
    for (e, mut n, mut node, mut visibility, mut colour) in &mut numbers {
        n.age += time.delta_secs() * playback.speed;
        if n.age >= RISE_NUMBER {
            commands.entity(e).despawn();
            continue;
        }
        let world = n.at + Vec3::Y * (0.4 * n.age / RISE_NUMBER + 0.3);
        if let Ok(s) = camera.world_to_viewport(at, world) {
            node.left = Val::Px(s.x - 10.0);
            node.top = Val::Px(s.y - 10.0);
            *visibility = Visibility::Inherited;
        }
        colour.0 = colour.0.with_alpha(1.0 - n.age / RISE_NUMBER);
    }
}

fn projectiles(
    mut commands: Commands,
    time: Res<Time<Real>>,
    playback: Res<Playback>,
    mut shots: Query<(Entity, &mut Projectile, &mut Transform)>,
) {
    for (e, mut p, mut transform) in &mut shots {
        p.age += time.delta_secs() * playback.speed;
        let t = (p.age / PROJECTILE).min(1.0);
        transform.translation = p.from.lerp(p.to, t) + Vec3::Y * 0.6 * 4.0 * t * (1.0 - t);
        if t >= 1.0 {
            commands.entity(e).despawn();
        }
    }
}

/// The next round may come once the last beat has played and HOLD more has passed, unless paused.
fn gate(show: Res<Show>, mut playback: ResMut<Playback>, mut gate: ResMut<Gate>) {
    let settled = !show.playing() && show.idle >= HOLD / playback.speed;
    let open = settled && (!playback.paused || playback.step);
    if open && playback.step {
        playback.step = false;
    }
    gate.open = open || (playback.skip && !playback.paused);
}
