//! The wormhole hub pages (Thera, Turnur) behind a left nav: state, periodic refresh, filters,
//! hunting spots and view. Both pages share the data, the filters and the splitter; each keeps
//! its own selection, sort and spot list.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeDelta, Utc};
use rune::tok::TextStyle;
use rune::{
    Align, Color, Cursor, Cx, Drag, El, Icon, KeyEvent, Phase, TextEdit, Tx, Waker, WindowHandle, col, el, icon, row,
    text,
};
use rune_kit::shell::{self, PaneState, Sidebar, SidebarRow, SplitView, Toolbar};
use rune_kit::{
    Dialog,
    Badge, BadgeVariant, Button, ButtonVariant, Cell, Checkbox, Kit, SegmentedControl, SortDir, Table, TableColumn,
    TableState, TextInput,
};

use crate::api::{self, Hub, Signature, Space};
use crate::settings::{self, PageSorts, Settings, ThemeChoice, WindowPlace};
use crate::universe::{self, Hunt, Spot, SpotSecurity, Stats};

/// Time between two EVE-Scout requests.
const REFRESH_EVERY: Duration = Duration::from_secs(60);
/// ESI stats change once an hour; they are re-read every 10 minutes.
const STATS_EVERY: Duration = Duration::from_secs(600);
/// After an ESI failure, retry sooner.
const STATS_RETRY: Duration = Duration::from_secs(60);
/// How long a connection that just appeared stays marked "NEW".
const NEW_FOR: TimeDelta = TimeDelta::minutes(30);
/// Displayed durations (remaining, age) are recomputed at this pace.
const CLOCK_TICK: f64 = 20.0;
/// Range of the hunting spot search, in stargate jumps.
pub const HUNT_RANGE: u32 = 10;
/// Above this, a spot is highlighted (NPC kills in the last hour).
const HOT_NPC: u32 = 200;

/// Splitter: default height of the hunting section (connection details, spots header and
/// table), and the smallest each side may get.
const SPLIT_DEFAULT: f32 = 360.0;
const SPLIT_MIN_BOTTOM: f32 = 150.0;
const SPLIT_MIN_TOP: f32 = 120.0;
const SPLITTER_H: f32 = 10.0;

const SIZES: [&str; 5] = ["Any", "Medium+", "Large+", "XL+", "Capital"];

/// The pages, in nav order.
const HUBS: [Hub; 2] = [Hub::Thera, Hub::Turnur];

static THERA_ICON: Icon = Icon {
    name: "hub-thera",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="5.25" stroke="currentColor" stroke-width="1.3" fill="none"/><circle cx="8" cy="8" r="2.25" stroke="currentColor" stroke-width="1.3" fill="none"/>"#,
};
static TURNUR_ICON: Icon = Icon {
    name: "hub-turnur",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="5.25" stroke="currentColor" stroke-width="1.3" fill="none"/><circle cx="8" cy="8" r="1.6" fill="currentColor"/>"#,
};

fn hub_name(hub: Hub) -> &'static str {
    match hub {
        Hub::Thera => "Thera",
        Hub::Turnur => "Turnur",
    }
}

fn hub_icon(hub: Hub) -> &'static Icon {
    match hub {
        Hub::Thera => &THERA_ICON,
        Hub::Turnur => &TURNUR_ICON,
    }
}

enum Msg {
    Signatures(Result<Vec<Signature>, String>),
    Stats(Result<Stats, String>),
}

/// What one hub page keeps for itself.
pub struct Page {
    hub: Hub,
    /// The connections shown (this hub's, filtered, then sorted).
    rows: Vec<Signature>,
    table: TableState,
    sort: (String, SortDir),
    selected: Option<String>,
    /// The selected connection's spots, sorted.
    spots: Vec<Spot>,
    spot_table: TableState,
    spot_sort: (String, SortDir),
    spot_selected: Option<u64>,
}

impl Page {
    fn new(hub: Hub) -> Page {
        let mut table = TableState::new();
        table.set_sort("age", SortDir::Ascending);
        let mut spot_table = TableState::new();
        spot_table.set_sort("npc", SortDir::Descending);
        Page {
            hub,
            rows: Vec::new(),
            table,
            sort: ("age".into(), SortDir::Ascending),
            selected: None,
            spots: Vec::new(),
            spot_table,
            spot_sort: ("npc".into(), SortDir::Descending),
            spot_selected: None,
        }
    }
}

pub struct State {
    /// Every connection received at the last refresh, both hubs.
    all: Vec<Signature>,
    pages: [Page; 2],
    /// Index of the page shown, in `HUBS`.
    current: usize,
    nav: PaneState,

    // Filters (shared by the pages)
    query: TextEdit,
    high: bool,
    low: bool,
    null: bool,
    wormhole: bool,
    min_size: usize,
    only_new: bool,

    // Hunting spots
    stats: Option<Stats>,
    stats_at: Option<DateTime<Utc>>,
    stats_error: Option<String>,
    spot_security: SpotSecurity,
    /// Per connection (id): active systems within range.
    hunts: HashMap<String, Hunt>,

    // Splitter between the connections and the hunting section
    bottom_h: f32,
    /// Height of the area the splitter divides, from its last layout.
    split_area_h: Option<f32>,

    // Settings kept between runs
    theme: ThemeChoice,
    /// The settings dialog is open.
    settings_open: bool,
    /// Where the window was (and returns to when un-maximized), as last saved.
    window_place: Option<WindowPlace>,
    /// What was last written to the settings file.
    saved: Settings,

    // New connection tracking
    loaded_once: bool,
    first_seen: HashMap<String, DateTime<Utc>>,

    // Refresh
    now: DateTime<Utc>,
    last_tick: f64,
    last_update: Option<DateTime<Utc>>,
    loading: bool,
    error: Option<String>,
    flash: Option<String>,
    inbox: Arc<Mutex<Vec<Msg>>>,
    refresh_tx: Option<Sender<()>>,
}

impl State {
    fn page(&self) -> &Page {
        &self.pages[self.current]
    }

    fn page_mut(&mut self) -> &mut Page {
        &mut self.pages[self.current]
    }
}

/// Defaults: headless renders and tests start from these, never from the user's file.
pub fn init() -> State {
    init_with(&Settings::default())
}

/// The state the app opens with, from the saved settings.
pub fn init_with(saved: &Settings) -> State {
    let dir = |desc: bool| if desc { SortDir::Descending } else { SortDir::Ascending };
    let mut pages = HUBS.map(Page::new);
    for (page, sorts) in pages.iter_mut().zip(&saved.sorts) {
        let (col, desc) = &sorts.connections;
        page.sort = (col.clone(), dir(*desc));
        page.table.set_sort(col.clone(), dir(*desc));
        let (col, desc) = &sorts.spots;
        page.spot_sort = (col.clone(), dir(*desc));
        page.spot_table.set_sort(col.clone(), dir(*desc));
    }
    let mut nav = PaneState::new(180.0).range(140.0, 320.0);
    nav.set_width(saved.nav_width);
    nav.set_open(saved.nav_open);
    State {
        all: Vec::new(),
        pages,
        current: saved.page.min(HUBS.len() - 1),
        nav,
        query: TextEdit::new(saved.query.clone()),
        high: saved.high,
        low: saved.low,
        null: saved.null,
        wormhole: saved.wormhole,
        min_size: saved.min_size.min(SIZES.len() - 1),
        only_new: saved.only_new,
        stats: None,
        stats_at: None,
        stats_error: None,
        spot_security: SpotSecurity::from_index(saved.spot_security),
        hunts: HashMap::new(),
        bottom_h: if saved.bottom_h.is_finite() { saved.bottom_h } else { SPLIT_DEFAULT },
        split_area_h: None,
        theme: saved.theme,
        settings_open: false,
        window_place: saved.window,
        saved: saved.clone(),
        loaded_once: false,
        first_seen: HashMap::new(),
        now: Utc::now(),
        last_tick: 0.0,
        last_update: None,
        loading: true,
        error: None,
        flash: None,
        inbox: Arc::new(Mutex::new(Vec::new())),
        refresh_tx: None,
    }
}

// ---------------------------------------------------------------------------------------------
// Refresh

/// Starts the thread that polls EVE-Scout (at launch, then every `REFRESH_EVERY`, or right away
/// on "Refresh") and the ESI stats (every `STATS_EVERY`).
pub fn setup(s: &mut State, wake: Waker) {
    if crate::HEADLESS.load(Ordering::Relaxed) {
        return;
    }
    // Parse the map now rather than on the first spot computation.
    std::thread::spawn(|| {
        universe::universe();
    });
    let (tx, rx) = mpsc::channel::<()>();
    s.refresh_tx = Some(tx);
    let inbox = s.inbox.clone();
    std::thread::Builder::new()
        .name("eve-scout".into())
        .spawn(move || {
            let client = api::client();
            let mut stats_due = Instant::now();
            loop {
                let sigs = api::fetch(&client);
                inbox.lock().unwrap().push(Msg::Signatures(sigs));
                wake();
                if Instant::now() >= stats_due {
                    let stats = universe::fetch_stats(&client);
                    stats_due = Instant::now() + if stats.is_ok() { STATS_EVERY } else { STATS_RETRY };
                    inbox.lock().unwrap().push(Msg::Stats(stats));
                    wake();
                }
                match rx.recv_timeout(REFRESH_EVERY) {
                    Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .expect("refresh thread");
}

/// Before every frame: takes the thread's results and moves the clock forward.
pub fn poll(s: &mut State, t: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    if t - s.last_tick >= CLOCK_TICK {
        s.last_tick = t;
        if !crate::HEADLESS.load(Ordering::Relaxed) {
            s.now = Utc::now();
            changed = true;
        }
    }
    tx.wake_after(Duration::from_secs_f64(CLOCK_TICK));

    let msgs = std::mem::take(&mut *s.inbox.lock().unwrap());
    for msg in msgs {
        let now = Utc::now();
        s.now = now;
        match msg {
            Msg::Signatures(Ok(list)) => {
                s.loading = false;
                merge(s, list, now);
            }
            Msg::Signatures(Err(e)) => {
                s.loading = false;
                s.error = Some(e);
            }
            Msg::Stats(Ok(stats)) => {
                s.stats = Some(stats);
                s.stats_at = Some(now);
                s.stats_error = None;
                recompute_hunts(s);
            }
            Msg::Stats(Err(e)) => s.stats_error = Some(e),
        }
        changed = true;
    }
    if changed {
        update_title(s, tx);
    }
    save_if_changed(s);
    changed
}

// ---------------------------------------------------------------------------------------------
// Settings kept between runs

thread_local! {
    /// The app's window, to read its place when it closes.
    static WINDOW: std::cell::RefCell<Option<WindowHandle>> = const { std::cell::RefCell::new(None) };
}

/// `App::on_window`: keep the window's handle.
pub fn remember_window(w: &WindowHandle) {
    WINDOW.with(|cell| *cell.borrow_mut() = Some(w.clone()));
}

/// The settings as they stand.
fn snapshot(s: &State) -> Settings {
    let desc = |d: SortDir| d == SortDir::Descending;
    Settings {
        window: s.window_place,
        page: s.current,
        query: s.query.text.clone(),
        high: s.high,
        low: s.low,
        null: s.null,
        wormhole: s.wormhole,
        min_size: s.min_size,
        only_new: s.only_new,
        spot_security: s.spot_security.index(),
        bottom_h: s.bottom_h,
        nav_width: s.nav.width(),
        nav_open: s.nav.is_open(),
        sorts: s
            .pages
            .iter()
            .map(|p| PageSorts { connections: (p.sort.0.clone(), desc(p.sort.1)), spots: (p.spot_sort.0.clone(), desc(p.spot_sort.1)) })
            .collect(),
        theme: s.theme,
    }
}

/// The filters at their defaults: search, security, ship size, new only, spot security.
fn filters_are_default(s: &State) -> bool {
    let d = Settings::default();
    s.query.text.is_empty()
        && (s.high, s.low, s.null, s.wormhole) == (d.high, d.low, d.null, d.wormhole)
        && s.min_size == d.min_size
        && s.only_new == d.only_new
        && s.spot_security.index() == d.spot_security
}

/// "Reset filters": every filter back to its default. Sorts, splitter and window stay.
fn reset_filters(s: &mut State) {
    let d = Settings::default();
    s.query = TextEdit::new("");
    (s.high, s.low, s.null, s.wormhole) = (d.high, d.low, d.null, d.wormhole);
    s.min_size = d.min_size;
    s.only_new = d.only_new;
    s.spot_security = SpotSecurity::from_index(d.spot_security);
    recompute_hunts(s);
}

/// Writes the settings when they changed since the last write (never in headless runs).
fn save_if_changed(s: &mut State) {
    if crate::HEADLESS.load(Ordering::Relaxed) {
        return;
    }
    let now = snapshot(s);
    if now != s.saved {
        if let Err(e) = settings::save(&now) {
            eprintln!("could not save settings to {}: {e}", settings::path().display());
        }
        s.saved = now;
    }
}

/// `App::on_close`: note where the window is, then save everything.
pub fn on_close(s: &mut State, _: &mut Tx) {
    let placement = WINDOW.with(|cell| cell.borrow().as_ref().and_then(|w| w.placement()));
    if let Some(p) = placement.filter(|p| !p.minimized && !p.full_screen) {
        let place = WindowPlace {
            x: p.position.0,
            y: p.position.1,
            width: p.content_size.0,
            height: p.content_size.1,
            maximized: p.maximized,
        };
        s.window_place = match (p.maximized, s.window_place) {
            // Maximized: keep the size it was before, which un-maximizing returns to.
            (true, Some(before)) => Some(WindowPlace { maximized: true, ..before }),
            _ => Some(place).filter(|x| x.is_usable() || x.maximized),
        };
    }
    save_if_changed(s);
}

fn update_title(s: &State, tx: &mut Tx) {
    let n = new_count(s, None);
    let page = hub_name(s.page().hub);
    let title = if n > 0 { format!("{APP_NAME} — {page} ({n} new)") } else { format!("{APP_NAME} — {page}") };
    tx.set_title(&title);
}

/// ⇧Ctrl+B shows and hides the nav.
pub fn on_key(s: &mut State, ev: &KeyEvent, _: &mut Tx) -> bool {
    if shell::is_sidebar_toggle(ev) {
        s.nav.toggle();
        return true;
    }
    false
}

/// Replaces the list with the one received, noting connections that weren't there before.
fn merge(s: &mut State, list: Vec<Signature>, now: DateTime<Utc>) {
    if s.loaded_once {
        let known: HashSet<&str> = s.all.iter().map(|x| x.id.as_str()).collect();
        let fresh: Vec<String> = list.iter().filter(|x| !known.contains(x.id.as_str())).map(|x| x.id.clone()).collect();
        for id in fresh {
            s.first_seen.insert(id, now);
        }
    }
    let ids: HashSet<&str> = list.iter().map(|x| x.id.as_str()).collect();
    s.first_seen.retain(|id, _| ids.contains(id.as_str()));
    s.all = list;
    s.loaded_once = true;
    s.last_update = Some(now);
    s.error = None;
    recompute_hunts(s);
}

fn request_refresh(s: &mut State) {
    if let Some(tx) = &s.refresh_tx {
        s.loading = true;
        let _ = tx.send(());
    }
}

fn show_page(s: &mut State, i: usize, tx: &mut Tx) {
    if s.current != i {
        s.current = i;
        s.flash = None;
        update_title(s, tx);
    }
}

// ---------------------------------------------------------------------------------------------
// Hunting spots

/// Recomputes the active systems within range of every k-space exit.
fn recompute_hunts(s: &mut State) {
    s.hunts.clear();
    if let Some(stats) = &s.stats {
        let u = universe::universe();
        for sig in s.all.iter().filter(|x| x.space() != Space::Wormhole) {
            let h = universe::hunt(u, stats, sig.in_system_id, HUNT_RANGE, s.spot_security);
            s.hunts.insert(sig.id.clone(), h);
        }
    }
    apply(s);
}

fn refresh_spots(s: &mut State, i: usize) {
    let spots = s.pages[i].selected.as_ref().and_then(|id| s.hunts.get(id)).map(|h| h.spots.clone()).unwrap_or_default();
    let page = &mut s.pages[i];
    page.spots = spots;
    let (col, dir) = page.spot_sort.clone();
    sort_spots(page, &col, dir);
    if page.spot_selected.is_some_and(|id| !page.spots.iter().any(|x| x.id == id)) {
        page.spot_selected = None;
    }
}

fn sort_spots(page: &mut Page, col: &str, dir: SortDir) {
    page.spot_sort = (col.to_string(), dir);
    page.spots.sort_by(|a, b| {
        let o = match col {
            "system" => rune_kit::natural_cmp(&a.name, &b.name),
            "sec" => a.sec.total_cmp(&b.sec),
            "region" => a.region.cmp(&b.region),
            "distance" => a.distance.cmp(&b.distance),
            "npc" => a.act.npc_kills.cmp(&b.act.npc_kills),
            "jumps" => a.act.jumps.cmp(&b.act.jumps),
            "ships" => a.act.ship_kills.cmp(&b.act.ship_kills),
            "pods" => a.act.pod_kills.cmp(&b.act.pod_kills),
            _ => std::cmp::Ordering::Equal,
        };
        dir.apply(o.then(a.distance.cmp(&b.distance)))
    });
}

fn select_connection(s: &mut State, id: &str) {
    let i = s.current;
    if s.pages[i].selected.as_deref() != Some(id) {
        s.pages[i].selected = Some(id.to_string());
        s.pages[i].spot_selected = None;
        refresh_spots(s, i);
    }
}

// ---------------------------------------------------------------------------------------------
// Splitter

/// The tallest the hunting section may be, so the connections keep `SPLIT_MIN_TOP`.
fn max_bottom(s: &State) -> f32 {
    s.split_area_h.map_or(f32::MAX, |h| (h - SPLITTER_H - SPLIT_MIN_TOP).max(SPLIT_MIN_BOTTOM))
}

/// The hunting section's height as drawn: what the splitter set, within what the window allows.
fn bottom_height(s: &State) -> f32 {
    s.bottom_h.clamp(SPLIT_MIN_BOTTOM, max_bottom(s))
}

fn drag_splitter(s: &mut State, d: &mut Drag) {
    match d.phase {
        // Remember the height at the press; moves are relative to it.
        Phase::Down => d.data = bottom_height(s),
        Phase::Move | Phase::Up => s.bottom_h = (d.data - d.dy).clamp(SPLIT_MIN_BOTTOM, max_bottom(s)),
    }
}

// ---------------------------------------------------------------------------------------------
// Connection filters and sorting

fn is_new(s: &State, id: &str) -> bool {
    s.first_seen.get(id).is_some_and(|t| s.now - *t < NEW_FOR)
}

/// New connections of one hub, or of both.
fn new_count(s: &State, hub: Option<Hub>) -> usize {
    s.all.iter().filter(|x| hub.is_none_or(|h| x.hub() == h) && is_new(s, &x.id)).count()
}

fn keep(s: &State, hub: Hub, sig: &Signature, query: &str) -> bool {
    let space_ok = match sig.space() {
        Space::High => s.high,
        Space::Low => s.low,
        Space::Null => s.null,
        Space::Wormhole => s.wormhole,
    };
    let size_ok = s.min_size == 0 || sig.size_rank() >= s.min_size as u8;
    let new_ok = !s.only_new || is_new(s, &sig.id);
    let query_ok = query.is_empty()
        || [&sig.in_system_name, &sig.in_region_name, &sig.wh_type, &sig.out_signature, &sig.in_signature]
            .iter()
            .any(|f| f.to_lowercase().contains(query))
        // A connection is also found by the name of a hunting spot within its range.
        || s.hunts.get(&sig.id).is_some_and(|h| h.spots.iter().any(|x| x.name.to_lowercase().contains(query)));
    sig.hub() == hub && space_ok && size_ok && new_ok && query_ok
}

/// Recomputes every page's rows from `all`, the filters and its sort.
fn apply(s: &mut State) {
    let query = s.query.text.trim().to_lowercase();
    for i in 0..s.pages.len() {
        let hub = s.pages[i].hub;
        let rows: Vec<Signature> = s.all.iter().filter(|x| keep(s, hub, x, &query)).cloned().collect();
        s.pages[i].rows = rows;
        let (col, dir) = s.pages[i].sort.clone();
        sort(s, i, &col, dir);
        let page = &mut s.pages[i];
        if page.selected.as_ref().is_some_and(|id| !page.rows.iter().any(|r| &r.id == id)) {
            page.selected = None;
        }
        refresh_spots(s, i);
    }
}

fn sort(s: &mut State, i: usize, col: &str, dir: SortDir) {
    let mut rows = std::mem::take(&mut s.pages[i].rows);
    let new: HashSet<String> = rows.iter().filter(|r| is_new(s, &r.id)).map(|r| r.id.clone()).collect();
    let best = |id: &str| s.hunts.get(id).and_then(|h| h.best()).map_or(0, |x| x.act.npc_kills);
    let total = |id: &str| s.hunts.get(id).map_or(0, |h| h.total_npc);
    rows.sort_by(|a, b| {
        let o = match col {
            "state" => new.contains(&b.id).cmp(&new.contains(&a.id)),
            "system" => rune_kit::natural_cmp(&a.in_system_name, &b.in_system_name),
            "class" => a.in_system_class.cmp(&b.in_system_class),
            "region" => a.in_region_name.cmp(&b.in_region_name),
            "hub_sig" => a.out_signature.cmp(&b.out_signature),
            "in_sig" => a.in_signature.cmp(&b.in_signature),
            "size" => a.size_rank().cmp(&b.size_rank()),
            "type" => a.wh_type.cmp(&b.wh_type),
            "remaining" => a.expires_at.cmp(&b.expires_at),
            "age" => b.created_at.cmp(&a.created_at),
            "best" => best(&a.id).cmp(&best(&b.id)),
            "npc_total" => total(&a.id).cmp(&total(&b.id)),
            _ => std::cmp::Ordering::Equal,
        };
        dir.apply(o)
    });
    let page = &mut s.pages[i];
    page.rows = rows;
    page.sort = (col.to_string(), dir);
}

// ---------------------------------------------------------------------------------------------
// Formatting

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn duration(d: TimeDelta) -> String {
    let m = d.num_minutes().max(0);
    if m >= 60 { format!("{}h {:02}m", m / 60, m % 60) } else { format!("{m}m") }
}

fn ago(now: DateTime<Utc>, t: DateTime<Utc>) -> String {
    let secs = (now - t).num_seconds().max(0);
    if secs < 60 { format!("{secs}s") } else { duration(TimeDelta::seconds(secs)) }
}

fn space_color(k: &Kit, space: Space) -> Color {
    match space {
        Space::High => k.c.success.icon,
        Space::Low => k.c.warning.icon,
        Space::Null => k.c.danger.icon,
        Space::Wormhole => k.c.info.icon,
    }
}

fn remaining_color(k: &Kit, left: TimeDelta) -> Option<Color> {
    if left < TimeDelta::hours(2) {
        Some(k.c.danger.icon)
    } else if left < TimeDelta::hours(4) {
        Some(k.c.warning.icon)
    } else {
        None
    }
}

fn jumps_label(d: u32) -> String {
    match d {
        0 => "exit".into(),
        1 => "1 jump".into(),
        n => format!("{n} jumps"),
    }
}

/// Dotlan's region map with the system selected and the NPC kills layer on
/// (`/map/Detorid/L8-WNE#npc`); Anoik.is for wormholes, which Dotlan doesn't map.
fn map_url(name: &str, region: &str, space: Space) -> String {
    let dotlan = |s: &str| s.replace(' ', "_");
    match space {
        Space::Wormhole => format!("https://anoik.is/systems/{name}"),
        _ => format!("https://evemaps.dotlan.net/map/{}/{}#npc", dotlan(region), dotlan(name)),
    }
}

fn zkill_url(system_id: u64) -> String {
    format!("https://zkillboard.com/system/{system_id}/")
}

// ---------------------------------------------------------------------------------------------
// View

/// Shown in the titlebar.
pub const APP_NAME: &str = "Eve Hunting";

/// The titlebar's height: the toolbar band, which doubles as the window's titlebar.
pub const TITLEBAR_HEIGHT: f32 = rune_kit::shell::TOOLBAR_HEIGHT;

static CROSSHAIR: Icon = Icon {
    name: "app-crosshair",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="5" stroke="currentColor" stroke-width="1.4" fill="none"/><path d="M8 1.5V4.5M8 11.5V14.5M1.5 8H4.5M11.5 8H14.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/><circle cx="8" cy="8" r="1.2" fill="currentColor"/>"#,
};

/// The app's name at the titlebar's leading end.
fn brand(k: &Kit) -> El {
    row()
        .key("brand")
        .gap(7.0)
        .center_y()
        .child(icon(&CROSSHAIR, 16.0).color(k.c.warning.icon))
        .child(text(APP_NAME).font(k.ty.ui_medium).color(k.c.text_base))
}

pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    let k = Kit::neutral(s.theme.is_dark(cx.system_dark()));
    // The caption buttons follow the app's theme, not the system's.
    cx.caption_color(k.c.text_base);
    let hub = hub_name(s.page().hub);
    // The toolbar is the window's titlebar: it drags the window, and on Windows Rune draws the
    // minimize, maximize and close buttons over its trailing end, which the actions keep clear of.
    let caption = el().key("caption-space").w(cx.chrome_insets().trailing).h(1.0);
    let toolbar = Toolbar::new()
        .leading(brand(&k))
        .title(format!("{hub} connections"))
        .trailing(toolbar_actions(s, &k))
        .trailing(caption);
    let body = SplitView::new(hub_page(s, &k))
        .sidebar(&s.nav, |s: &mut State| &mut s.nav, nav(s, &k))
        .toolbar(toolbar)
        .titlebar_inset(cx.titlebar_inset())
        .reduced_motion(cx.is_reduced_motion())
        .build(&k);
    let mut layer = Vec::new();
    settings_dialog(s, &k, &mut layer);
    rune_kit::root(body, layer)
}

static GEAR: Icon = Icon {
    name: "app-settings",
    view: 16.0,
    body: r#"<path d="M3 5H13M3 11H13" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/><circle cx="6" cy="5" r="1.6" fill="currentColor"/><circle cx="10.5" cy="11" r="1.6" fill="currentColor"/>"#,
};

/// The settings dialog: theme, and resetting the filters.
fn settings_dialog(s: &State, k: &Kit, layer: &mut Vec<El>) {
    let section = |title: &str, hint: &str, control: El| {
        row()
            .w_full()
            .gap(16.0)
            .py(10.0)
            .center_y()
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(2.0)
                    .child(text(title.to_string()).font(k.ty.ui_medium).color(k.c.text_base))
                    .child(text(hint.to_string()).font(k.ty.small).color(k.c.text_muted).wrap()),
            )
            .child(control)
    };
    let themes = ThemeChoice::ALL.map(ThemeChoice::label);
    let theme = SegmentedControl::new(&themes, Some(s.theme.index()))
        .label("Theme")
        .fit()
        .on_pick(|s: &mut State, i, _| s.theme = ThemeChoice::from_index(i))
        .build(k)
        .key("theme");
    let reset = Button::new("Reset filters")
        .variant(ButtonVariant::Neutral)
        .disabled(filters_are_default(s))
        .on_press(reset_filters)
        .build(k)
        .key("reset-filters");
    let body = col()
        .key("settings-body")
        .w_full()
        .px(16.0)
        .pb(16.0)
        .child(section("Theme", "System follows Windows' light or dark mode.", theme))
        .child(section(
            "Filters",
            "Search, security, ship size, New only and the hunting spots' security, on both pages.",
            reset,
        ))
        .child(
            text(format!("Saved in {}", settings::path().display()))
                .font(k.ty.small)
                .color(k.c.text_faint)
                .truncate()
                .pt(8.0),
        );
    Dialog::new("settings", "Settings")
        .open(s.settings_open)
        .width(520.0)
        .fit()
        .on_close(|s: &mut State| s.settings_open = false)
        .child(body)
        .build(k, layer);
}

/// The left nav: one row per hub, with its connection count and new connections, and the
/// settings at the bottom.
fn nav(s: &State, k: &Kit) -> El {
    let settings = SidebarRow::new("Settings")
        .icon(&GEAR)
        .height(32.0)
        .on_select(|s: &mut State, _| s.settings_open = true)
        .build(k)
        .key("nav-settings");
    HUBS.iter()
        .enumerate()
        .fold(Sidebar::new().label("Hubs").footer(settings), |sb, (i, &hub)| {
            let count = s.all.iter().filter(|x| x.hub() == hub).count();
            let mut r = SidebarRow::new(hub_name(hub))
                .icon(hub_icon(hub))
                .height(32.0)
                .selected(s.current == i)
                .on_select(move |s: &mut State, tx| show_page(s, i, tx));
            match new_count(s, Some(hub)) {
                0 => r = r.detail(count.to_string()),
                n => r = r.badge(format!("{n} new")),
            }
            sb.child(r.build(k).key(format!("nav-{}", hub_name(hub).to_lowercase())))
        })
        .build(k)
}

fn toolbar_actions(s: &State, k: &Kit) -> El {
    let mut bar = row().gap(6.0).center_y();
    if new_count(s, Some(s.page().hub)) > 0 {
        bar = bar.child(
            Button::new("Mark all as seen")
                .variant(ButtonVariant::Ghost)
                .on_press(|s: &mut State| {
                    let hub = s.page().hub;
                    let seen: Vec<String> = s.all.iter().filter(|x| x.hub() == hub).map(|x| x.id.clone()).collect();
                    for id in seen {
                        s.first_seen.remove(&id);
                    }
                    apply(s);
                })
                .build(k),
        );
    }
    bar.child(
        Button::new("Refresh")
            .variant(ButtonVariant::Neutral)
            .disabled(s.refresh_tx.is_none())
            .on_press(request_refresh)
            .build(k),
    )
}

fn hub_page(s: &State, k: &Kit) -> El {
    let hub = hub_name(s.page().hub).to_lowercase();
    let frame = |key: String, child: El| {
        col()
            .key(key)
            .w_full()
            .min_h(0.0)
            .radius(k.radius.md)
            .fill(k.c.bg_base)
            .clip_rounded(k.radius.md)
            .child(child)
    };
    let hunting = col()
        .key("hunting")
        .w_full()
        .h(bottom_height(s))
        .min_h(0.0)
        .gap(6.0)
        .child(details(s, k))
        .child(spots_header(s, k))
        .child(frame(format!("spots-{hub}"), spots(s, k)).grow(1.0));
    // The splitter divides this area between the connections and the hunting section.
    let split = col()
        .key("split")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .on_layout(|s: &mut State, r| {
            if s.split_area_h != Some(r[3]) {
                s.split_area_h = Some(r[3]);
            }
        })
        .child(frame(format!("conns-{hub}"), connections(s, k)).grow(1.0))
        .child(splitter(k))
        .child(hunting);
    col()
        .key("page")
        .w_full()
        .h_full()
        .padding(12.0)
        .gap(10.0)
        .child(status(s, k))
        .child(filters(s, k))
        .child(split)
}

/// A bar between the two tables: drag it to share the height, double-click to reset.
fn splitter(k: &Kit) -> El {
    el().key("splitter")
        .w_full()
        .h(SPLITTER_H)
        .center()
        .cursor(Cursor::ResizeRow)
        .role("separator")
        .label("Resize the connections and hunting spots")
        .on_drag(|s: &mut State, d: &mut Drag, _: &mut Tx| drag_splitter(s, d))
        .on_double_press(|s: &mut State| s.bottom_h = SPLIT_DEFAULT)
        .child(el().w(44.0).h(4.0).radius(2.0).fill(k.c.border_strong))
}

fn status(s: &State, k: &Kit) -> El {
    let hub = s.page().hub;
    let n_new = new_count(s, Some(hub));
    let count = s.all.iter().filter(|x| x.hub() == hub).count();
    let line = match (&s.error, s.last_update, s.loading) {
        (Some(e), _, _) => text(format!("Error: {e}")).color(k.c.danger.icon),
        (None, None, _) => text("Loading EVE-Scout data…").color(k.c.text_muted),
        (None, Some(t), loading) => {
            let stats = match (&s.stats_error, s.stats_at) {
                (Some(_), _) => " · ESI stats unavailable".to_string(),
                (None, Some(at)) => format!(" · ESI stats {} ago", ago(s.now, at)),
                (None, None) => " · loading ESI stats…".to_string(),
            };
            text(format!(
                "{count} connection{} · updated {} ago{}{}",
                plural(count),
                ago(s.now, t),
                if loading { " · refreshing…" } else { "" },
                stats
            ))
            .color(k.c.text_muted)
        }
    }
    .font(k.ty.text_small)
    .key("status");
    let mut bar = row().key("status-bar").w_full().center_y().gap(10.0);
    if n_new > 0 {
        bar = bar.child(Badge::new(format!("{n_new} new")).variant(BadgeVariant::Accent).build(k));
    }
    bar.child(line)
}

fn filters(s: &State, k: &Kit) -> El {
    let search = TextInput::new(&s.query)
        .placeholder("System, region, sig, spot…")
        .leading_icon(k.glyph.search)
        .clear_button()
        .on_edit(|s: &mut State| &mut s.query)
        .on_change(|s: &mut State, _| apply(s))
        .build(k);
    let size = SegmentedControl::new(&SIZES, Some(s.min_size))
        .label("Minimum ship size")
        .fit()
        .on_pick(|s: &mut State, i, _| {
            s.min_size = i;
            apply(s);
        })
        .build(k);
    let check = |label: &str, on: bool, f: fn(&mut State)| {
        Checkbox::new(on)
            .label(label)
            .on_toggle(move |s: &mut State| {
                f(s);
                apply(s);
            })
            .build(k)
    };
    let spaces = row()
        .gap(14.0)
        .center_y()
        .child(check("High-sec", s.high, |s| s.high ^= true))
        .child(check("Low-sec", s.low, |s| s.low ^= true))
        .child(check("Null-sec", s.null, |s| s.null ^= true))
        .child(check("W-space", s.wormhole, |s| s.wormhole ^= true))
        .child(check("New only", s.only_new, |s| s.only_new ^= true));
    let shown = s.page().rows.len();
    col()
        .key("filters")
        .w_full()
        .gap(10.0)
        .child(row().w_full().gap(12.0).center_y().child(search).child(size))
        .child(
            row()
                .w_full()
                .center_y()
                .child(spaces)
                .child(el().grow(1.0))
                .child(text(format!("{shown} shown")).font(k.ty.text_small).color(k.c.text_muted)),
        )
}

/// The signature to scan down in the hub, drawn as an amber pill so it stands out in the row.
fn hub_sig_pill(k: &Kit, sig: &str) -> El {
    let style = TextStyle { weight: 650.0, ..k.ty.mono };
    row()
        .h(20.0)
        .px(7.0)
        .center_y()
        .radius(4.0)
        .fill(k.c.warning.bg)
        .child(text(sig.to_string()).font(style).color(k.c.warning.fg))
}

fn connections(s: &State, k: &Kit) -> El {
    let ck = k.clone();
    let hub = hub_name(s.page().hub);
    Table::new(
        |s: &State| &s.page().table,
        |s: &mut State| &mut s.page_mut().table,
        |s: &State| s.page().rows.len(),
        |s: &State, i| Cow::Borrowed(s.page().rows[i].id.as_str()),
    )
    .name(format!("{hub} connections"))
    .column(TableColumn::new("state", "").width(56.0).resizable(false).sortable())
    .column(TableColumn::new("system", "System").flex(1.0).min(96.0).sortable())
    .column(TableColumn::new("class", "Class").width(56.0).sortable())
    .column(TableColumn::new("region", "Region").flex(0.7).min(90.0).sortable())
    .column(TableColumn::new("hub_sig", format!("{hub} sig")).width(96.0).sortable())
    .column(TableColumn::new("in_sig", "Exit sig").width(84.0).sortable())
    .column(TableColumn::new("size", "Max size").width(82.0).sortable())
    .column(TableColumn::new("type", "Type").width(58.0).sortable())
    .column(TableColumn::new("remaining", "Remaining").width(94.0).align(Align::End).sortable())
    .column(TableColumn::new("age", "Age").width(72.0).align(Align::End).sortable())
    .column(TableColumn::new("best", format!("Best spot ≤{HUNT_RANGE} jumps")).flex(1.8).min(160.0).sortable())
    .column(TableColumn::new("npc_total", format!("NPC ≤{HUNT_RANGE}")).width(82.0).align(Align::End).sortable())
    .cell(move |s: &State, i, column| {
        let sig = &s.page().rows[i];
        let k = &ck;
        let hunt = s.hunts.get(&sig.id);
        match column {
            "state" if is_new(s, &sig.id) => Cell::el(Badge::new("NEW").variant(BadgeVariant::Accent).build(k)),
            "system" => Cell::text(&sig.in_system_name),
            "class" => Cell::text(sig.class_label()).color(space_color(k, sig.space())),
            "region" => Cell::text(&sig.in_region_name).muted(),
            "hub_sig" => Cell::el(hub_sig_pill(k, &sig.out_signature)),
            "in_sig" => Cell::text(&sig.in_signature).mono(),
            "size" => Cell::text(sig.size_label()),
            "type" => Cell::text(&sig.wh_type).mono().muted(),
            "remaining" => {
                let left = sig.expires_at - s.now;
                let c = Cell::text(duration(left)).mono();
                match remaining_color(k, left) {
                    Some(color) => c.color(color),
                    None => c,
                }
            }
            "age" => Cell::text(duration(s.now - sig.created_at)).mono().muted(),
            "best" => match (sig.space(), hunt.and_then(|h| h.best())) {
                (Space::Wormhole, _) => Cell::text("—").muted(),
                (_, Some(b)) => {
                    let c = Cell::text(format!("{} · {} NPC · {}", b.name, b.act.npc_kills, jumps_label(b.distance)));
                    if b.act.npc_kills >= HOT_NPC { c.color(k.c.text_accent) } else { c }
                }
                (_, None) if s.stats.is_some() => Cell::text("no activity").muted(),
                (_, None) => Cell::text("…").muted(),
            },
            "npc_total" => match hunt {
                Some(h) if sig.space() != Space::Wormhole => Cell::text(h.total_npc.to_string()).mono(),
                _ => Cell::text(""),
            },
            _ => Cell::text(""),
        }
    })
    .on_sort(|s: &mut State, col, dir, _| {
        let i = s.current;
        sort(s, i, col, dir);
    })
    .on_select(|s: &mut State, id, _| select_connection(s, id))
    .on_open(|s: &mut State, id, tx| {
        if let Some(name) = s.page().rows.iter().find(|r| r.id == id).map(|r| r.in_system_name.clone()) {
            tx.copy(name.clone());
            s.flash = Some(format!("\"{name}\" copied to the clipboard"));
        }
    })
    .zebra(true)
    .build(s, k)
    .grow(1.0)
    .min_h(0.0)
}

/// Copy name / Dotlan (or Anoik.is) / zKillboard buttons for a system.
fn system_actions(k: &Kit, name: &str, region: &str, id: u64, space: Space) -> El {
    let copy_name = name.to_string();
    let map = map_url(name, region, space);
    let zkill = zkill_url(id);
    let map_label = if space == Space::Wormhole { "Anoik.is" } else { "Dotlan" };
    row()
        .gap(6.0)
        .center_y()
        .child(
            Button::new("Copy name")
                .variant(ButtonVariant::Neutral)
                .on_press_tx(move |s: &mut State, tx| {
                    tx.copy(copy_name.clone());
                    s.flash = Some(format!("\"{copy_name}\" copied"));
                })
                .build(k),
        )
        .child(
            Button::new(map_label)
                .variant(ButtonVariant::Ghost)
                .on_press_tx(move |_: &mut State, tx| {
                    let _ = tx.open_url(&map);
                })
                .build(k),
        )
        .child(
            Button::new("zKillboard")
                .variant(ButtonVariant::Ghost)
                .on_press_tx(move |_: &mut State, tx| {
                    let _ = tx.open_url(&zkill);
                })
                .build(k),
        )
}

fn selected_signature(s: &State) -> Option<&Signature> {
    let page = s.page();
    page.selected.as_ref().and_then(|id| page.rows.iter().find(|r| &r.id == id))
}

fn details(s: &State, k: &Kit) -> El {
    let bar = row().key("details").w_full().h(32.0).min_h(32.0).center_y().gap(10.0);
    let Some(sig) = selected_signature(s) else {
        let hint = s
            .flash
            .clone()
            .unwrap_or_else(|| "Select a connection to see its hunting spots · double-click or Enter copies the system name".into());
        return bar.child(text(hint).font(k.ty.text_small).color(k.c.text_muted));
    };
    let summary = format!(
        "{} ({}, {}) · via {} · sig {} / {} · {} · expires in {}",
        sig.in_system_name,
        sig.class_label(),
        sig.in_region_name,
        sig.out_system_name,
        sig.out_signature,
        sig.in_signature,
        sig.size_label(),
        duration(sig.expires_at - s.now)
    );
    let mut bar = bar.child(text(summary).font(k.ty.text_small).color(k.c.text_base).truncate().grow(1.0).min_w(0.0));
    if let Some(flash) = &s.flash {
        bar = bar.child(text(flash.clone()).font(k.ty.text_small).color(k.c.text_muted));
    }
    bar.child(system_actions(k, &sig.in_system_name, &sig.in_region_name, sig.in_system_id, sig.space()))
}

fn spots_header(s: &State, k: &Kit) -> El {
    let sig = selected_signature(s);
    let title = match sig {
        Some(sig) => format!("Hunting spots ≤{HUNT_RANGE} jumps from {}", sig.in_system_name),
        None => format!("Hunting spots ≤{HUNT_RANGE} jumps"),
    };
    let info = match (sig, sig.and_then(|x| s.hunts.get(&x.id))) {
        (None, _) => String::new(),
        (Some(sig), _) if sig.space() == Space::Wormhole => "W-space exit: no stargates, no spots computed".into(),
        (Some(_), None) if s.stats_error.is_some() => "ESI stats unavailable".into(),
        (Some(_), None) => "Loading ESI stats…".into(),
        (Some(_), Some(h)) => format!(
            "{} system{} with NPC kills out of {} in range · NPC kills and jumps over the last hour",
            h.spots.len(),
            plural(h.spots.len()),
            h.reachable
        ),
    };
    let labels = SpotSecurity::ALL.map(SpotSecurity::label);
    let security = SegmentedControl::new(&labels, Some(s.spot_security.index()))
        .label("Hunting spot security")
        .fit()
        .on_pick(|s: &mut State, i, _| {
            s.spot_security = SpotSecurity::from_index(i);
            recompute_hunts(s);
        })
        .build(k);
    let mut bar = row()
        .key("spots-header")
        .w_full()
        .h(32.0)
        .min_h(32.0)
        .center_y()
        .gap(12.0)
        .child(text(title).font(k.ty.body_medium).color(k.c.text_base))
        .child(text(info).font(k.ty.text_small).color(k.c.text_muted).truncate().min_w(0.0).shrink())
        .child(el().grow(1.0))
        .child(security);
    let page = s.page();
    if let Some(spot) = page.spot_selected.and_then(|id| page.spots.iter().find(|x| x.id == id)) {
        bar = bar.child(system_actions(k, &spot.name, &spot.region, spot.id, spot.space));
    }
    bar
}

fn spot_id(row_id: &str) -> Option<u64> {
    row_id.strip_prefix("spot-").and_then(|x| x.parse().ok())
}

fn spots(s: &State, k: &Kit) -> El {
    let ck = k.clone();
    Table::new(
        |s: &State| &s.page().spot_table,
        |s: &mut State| &mut s.page_mut().spot_table,
        |s: &State| s.page().spots.len(),
        |s: &State, i| Cow::Owned(format!("spot-{}", s.page().spots[i].id)),
    )
    .name("Hunting spots")
    .column(TableColumn::new("system", "System").flex(1.0).min(110.0).sortable())
    .column(TableColumn::new("sec", "Sec").width(56.0).align(Align::End).sortable())
    .column(TableColumn::new("region", "Region").flex(1.0).min(100.0).sortable())
    .column(TableColumn::new("distance", "Distance").width(88.0).align(Align::End).sortable())
    .column(TableColumn::new("npc", "NPC kills 1h").width(124.0).align(Align::End).sortable())
    .column(TableColumn::new("jumps", "Jumps 1h").width(92.0).align(Align::End).sortable())
    .column(TableColumn::new("ships", "Ship kills").width(100.0).align(Align::End).sortable())
    .column(TableColumn::new("pods", "Pods").width(60.0).align(Align::End).sortable())
    .cell(move |s: &State, i, column| {
        let x = &s.page().spots[i];
        let k = &ck;
        match column {
            "system" => Cell::text(&x.name),
            "sec" => Cell::text(format!("{:.1}", x.sec)).mono().color(space_color(k, x.space)),
            "region" => Cell::text(&x.region).muted(),
            "distance" => Cell::text(jumps_label(x.distance)).mono(),
            "npc" => {
                let c = Cell::text(x.act.npc_kills.to_string()).mono();
                if x.act.npc_kills >= HOT_NPC { c.color(k.c.text_accent) } else { c }
            }
            "jumps" => Cell::text(x.act.jumps.to_string()).mono(),
            "ships" if x.act.ship_kills > 0 => Cell::text(x.act.ship_kills.to_string()).mono().color(k.c.danger.icon),
            "pods" if x.act.pod_kills > 0 => Cell::text(x.act.pod_kills.to_string()).mono().color(k.c.danger.icon),
            "ships" | "pods" => Cell::text("0").mono().muted(),
            _ => Cell::text(""),
        }
    })
    .on_sort(|s: &mut State, col, dir, _| sort_spots(s.page_mut(), col, dir))
    .on_select(|s: &mut State, id, _| s.page_mut().spot_selected = spot_id(id))
    .on_open(|s: &mut State, id, tx| {
        let id = spot_id(id);
        if let Some(name) = s.page().spots.iter().find(|x| Some(x.id) == id).map(|x| x.name.clone()) {
            tx.copy(name.clone());
            s.flash = Some(format!("\"{name}\" copied to the clipboard"));
        }
    })
    .zebra(true)
    .build(s, k)
    .grow(1.0)
    .min_h(0.0)
}

// ---------------------------------------------------------------------------------------------
// Headless scenes (`--scene demo`): frozen data from `fixtures/`.

pub const SCENES: &[(&str, fn(&mut State))] =
    &[
        ("demo", demo),
        ("demo-turnur", demo_turnur),
        ("demo-tall", demo_tall),
        ("demo-null", demo_null),
        ("demo-settings", demo_settings),
        ("demo-light", demo_light),
    ];

/// The settings dialog open, over filtered connections (so "Reset filters" is on).
fn demo_settings(s: &mut State) {
    s.null = false;
    demo(s);
    s.settings_open = true;
}

/// The light theme forced, whatever the system uses.
fn demo_light(s: &mut State) {
    s.theme = ThemeChoice::Light;
    demo(s);
}

/// Hunting spots in null-sec only.
fn demo_null(s: &mut State) {
    s.spot_security = SpotSecurity::Null;
    demo(s);
}

fn demo(s: &mut State) {
    let list = api::parse(include_str!("../fixtures/signatures.json")).unwrap_or_default();
    let now = list.iter().map(|x| x.created_at).max().unwrap_or_else(Utc::now) + TimeDelta::minutes(20);
    s.now = now;
    s.loading = false;
    let mut recent: Vec<&Signature> = list.iter().collect();
    recent.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    for sig in recent.iter().take(2) {
        s.first_seen.insert(sig.id.clone(), now - TimeDelta::minutes(3));
    }
    s.all = list;
    s.loaded_once = true;
    s.last_update = Some(now - TimeDelta::seconds(25));
    s.stats = universe::parse_stats(include_str!("../fixtures/system_kills.json"), include_str!("../fixtures/system_jumps.json")).ok();
    s.stats_at = Some(now - TimeDelta::minutes(4));
    recompute_hunts(s);
    // On every page, select the connection with the best spot.
    for i in 0..s.pages.len() {
        let page = &s.pages[i];
        let best = page
            .rows
            .iter()
            .max_by_key(|r| s.hunts.get(&r.id).and_then(|h| h.best()).map_or(0, |b| b.act.npc_kills))
            .map(|r| r.id.clone());
        if let Some(id) = best {
            s.pages[i].table.select(id.clone());
            let current = std::mem::replace(&mut s.current, i);
            select_connection(s, &id);
            s.current = current;
        }
    }
}

fn demo_turnur(s: &mut State) {
    demo(s);
    s.current = 1;
}

/// The splitter dragged up: the hunting section takes most of the window.
fn demo_tall(s: &mut State) {
    demo(s);
    s.bottom_h = 560.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded() -> State {
        let mut s = init();
        demo(&mut s);
        s
    }

    #[test]
    fn each_page_shows_only_its_hub() {
        let s = loaded();
        for (page, hub) in s.pages.iter().zip(HUBS) {
            assert!(!page.rows.is_empty(), "{} has connections", hub_name(hub));
            assert!(page.rows.iter().all(|r| r.hub() == hub));
        }
    }

    #[test]
    fn pages_keep_their_own_selection() {
        let mut s = loaded();
        let thera = s.pages[0].selected.clone();
        s.current = 1;
        let other = s.pages[1].rows.last().unwrap().id.clone();
        select_connection(&mut s, &other);
        assert_eq!(s.pages[0].selected, thera, "selecting on Turnur leaves Thera alone");
        assert_eq!(s.pages[1].selected.as_deref(), Some(other.as_str()));
        assert!(s.pages[1].spots.iter().all(|x| s.hunts[&other].spots.iter().any(|y| y.id == x.id)));
    }

    #[test]
    fn a_connection_that_appears_on_refresh_is_new() {
        let mut s = loaded();
        s.first_seen.clear();
        let mut list = s.all.clone();
        let mut extra = list[0].clone();
        extra.id = "fresh".into();
        list.push(extra);
        let now = s.now;
        merge(&mut s, list, now);
        assert!(is_new(&s, "fresh"));
        assert_eq!(new_count(&s, None), 1);
        let list: Vec<_> = s.all.iter().filter(|x| x.id != "fresh").cloned().collect();
        merge(&mut s, list, now);
        assert_eq!(new_count(&s, None), 0);
    }

    #[test]
    fn new_counts_are_per_hub() {
        let s = loaded();
        assert_eq!(new_count(&s, Some(Hub::Thera)) + new_count(&s, Some(Hub::Turnur)), new_count(&s, None));
    }

    #[test]
    fn filters_combine() {
        let mut s = loaded();
        s.high = false;
        s.low = false;
        s.wormhole = false;
        s.min_size = 3;
        apply(&mut s);
        for page in &s.pages {
            assert!(page.rows.iter().all(|r| r.space() == Space::Null && r.size_rank() >= 3));
        }
    }

    #[test]
    fn sorting_reorders_the_current_page_only() {
        let mut s = loaded();
        let turnur: Vec<String> = s.pages[1].rows.iter().map(|r| r.id.clone()).collect();
        sort(&mut s, 0, "npc_total", SortDir::Descending);
        let totals: Vec<u32> = s.pages[0].rows.iter().map(|r| s.hunts.get(&r.id).map_or(0, |h| h.total_npc)).collect();
        assert!(totals.windows(2).all(|w| w[0] >= w[1]));
        assert_eq!(turnur, s.pages[1].rows.iter().map(|r| r.id.clone()).collect::<Vec<_>>());
    }

    #[test]
    fn k_space_exits_get_hunting_spots_and_wormholes_none() {
        let s = loaded();
        for sig in &s.all {
            match sig.space() {
                Space::Wormhole => assert!(!s.hunts.contains_key(&sig.id)),
                _ => assert!(s.hunts.contains_key(&sig.id), "{} has no hunt", sig.in_system_name),
            }
        }
        assert!(!s.page().spots.is_empty(), "the demo selects a connection with spots");
        assert!(s.page().spots.iter().all(|x| x.distance <= HUNT_RANGE && x.space != Space::High));
    }

    #[test]
    fn the_spot_security_choice_filters_every_page() {
        let mut s = loaded();
        let count = |s: &State| s.hunts.values().map(|h| h.spots.len()).sum::<usize>();
        let mut counts = Vec::new();
        for sec in SpotSecurity::ALL {
            s.spot_security = sec;
            recompute_hunts(&mut s);
            assert!(s.hunts.values().flat_map(|h| &h.spots).all(|x| sec.allows(x.space)), "{sec:?}");
            assert!(s.pages.iter().flat_map(|p| &p.spots).all(|x| sec.allows(x.space)), "{sec:?}: the shown spots follow");
            counts.push(count(&s));
        }
        assert!(counts[0] <= counts[1] && counts[1] <= counts[2], "Null ⊂ Null + Low ⊂ All: {counts:?}");
    }

    #[test]
    fn dotlan_opens_the_region_map_on_the_system() {
        assert_eq!(map_url("L8-WNE", "Detorid", Space::Null), "https://evemaps.dotlan.net/map/Detorid/L8-WNE#npc");
        assert_eq!(map_url("Kador Prime", "Kador", Space::High), "https://evemaps.dotlan.net/map/Kador/Kador_Prime#npc");
        assert_eq!(map_url("Otsasai", "The Forge", Space::Low), "https://evemaps.dotlan.net/map/The_Forge/Otsasai#npc");
        assert_eq!(map_url("J131252", "D-R00020", Space::Wormhole), "https://anoik.is/systems/J131252");
    }

    #[test]
    fn settings_round_trip_through_the_state() {
        let saved = Settings {
            window: Some(WindowPlace { x: 40, y: 30, width: 1500, height: 900, maximized: true }),
            page: 1,
            query: "omist".into(),
            low: false,
            wormhole: false,
            min_size: 2,
            only_new: true,
            spot_security: SpotSecurity::Null.index(),
            theme: ThemeChoice::Dark,
            bottom_h: 420.0,
            nav_width: 220.0,
            nav_open: false,
            sorts: vec![
                PageSorts { connections: ("best".into(), true), spots: ("jumps".into(), true) },
                PageSorts { connections: ("system".into(), false), spots: ("distance".into(), false) },
            ],
            ..Settings::default()
        };
        let s = init_with(&saved);
        assert_eq!(s.current, 1);
        assert_eq!(s.query.text, "omist");
        assert_eq!(s.pages[0].table.sort(), Some(("best", SortDir::Descending)));
        assert_eq!(snapshot(&s), saved, "what is loaded is what would be saved");
    }

    #[test]
    fn reset_filters_restores_every_filter_and_nothing_else() {
        let mut s = loaded();
        s.query = TextEdit::new("omist");
        s.null = false;
        s.min_size = 3;
        s.only_new = true;
        s.spot_security = SpotSecurity::All;
        s.bottom_h = 500.0;
        s.theme = ThemeChoice::Light;
        sort(&mut s, 0, "best", SortDir::Descending);
        apply(&mut s);
        assert!(!filters_are_default(&s));
        reset_filters(&mut s);
        assert!(filters_are_default(&s));
        assert_eq!(s.pages[0].rows.len(), s.all.iter().filter(|x| x.hub() == Hub::Thera).count(), "every connection is back");
        assert_eq!((s.bottom_h, s.theme), (500.0, ThemeChoice::Light), "layout and theme stay");
        assert_eq!(s.pages[0].sort.0, "best", "sorts stay");
    }

    #[test]
    fn a_damaged_settings_file_cannot_break_the_state() {
        let saved = Settings { page: 9, min_size: 42, bottom_h: f32::NAN, ..Settings::default() };
        let s = init_with(&saved);
        assert!(s.current < HUBS.len());
        assert!(s.min_size < SIZES.len());
        assert!(bottom_height(&s).is_finite());
    }

    #[test]
    fn the_splitter_stays_within_the_window() {
        let mut s = loaded();
        s.split_area_h = Some(600.0);
        s.bottom_h = 5000.0;
        assert_eq!(bottom_height(&s), 600.0 - SPLITTER_H - SPLIT_MIN_TOP);
        s.bottom_h = 10.0;
        assert_eq!(bottom_height(&s), SPLIT_MIN_BOTTOM);
    }
}
