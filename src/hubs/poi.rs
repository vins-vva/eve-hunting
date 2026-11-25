//! The points of interest page: systems worth watching (an enemy staging, a trade hub…), each
//! with a number of gate jumps, and the Thera and Turnur connections within that many jumps.
//!
//! Hovering a point of interest lists its connections; clicking it keeps it listed. "Add" and
//! each row's "Edit" open a dialog with the system, its type, a comment and the jumps.

use super::*;
use crate::settings::{Poi, PoiKind};

/// The jumps a new point of interest starts with, and the most it may take.
pub const DEFAULT_JUMPS: u32 = 10;
pub const MAX_JUMPS: u32 = 40;

/// A point of interest, with the systems within its range.
pub struct PoiEntry {
    pub id: u64,
    pub kind: Option<PoiKind>,
    pub comment: String,
    pub jumps: u32,
    /// Gate jumps from each system within `jumps` of it.
    near: HashMap<u64, u32>,
}

impl PoiEntry {
    pub fn new(id: u64, kind: Option<PoiKind>, comment: &str, jumps: u32) -> PoiEntry {
        let jumps = jumps.min(MAX_JUMPS);
        PoiEntry { id, kind, comment: comment.trim().to_string(), jumps, near: universe::universe().within(id, jumps).into_iter().collect() }
    }

    pub fn name(&self) -> &'static str {
        system_name(self.id).unwrap_or("?")
    }

    /// Gate jumps from `system` to here, when within range.
    pub fn jumps_from(&self, system: u64) -> Option<u32> {
        self.near.get(&system).copied()
    }
}

/// The saved points of interest; names no longer on the map are dropped, a system listed twice
/// is kept once.
pub fn load(saved: &[Poi]) -> Vec<PoiEntry> {
    let u = universe::universe();
    let mut out: Vec<PoiEntry> = Vec::new();
    for p in saved {
        if let Some(id) = u.find(p.system.trim()).filter(|id| !out.iter().any(|x| x.id == *id)) {
            out.push(PoiEntry::new(id, p.kind, &p.comment, p.jumps));
        }
    }
    out
}

pub fn save(list: &[PoiEntry]) -> Vec<Poi> {
    list.iter().map(|p| Poi { system: p.name().to_string(), kind: p.kind, comment: p.comment.clone(), jumps: p.jumps }).collect()
}

/// The connections (both hubs) within range of `poi`, with their jumps to it.
pub fn matches<'a>(all: &'a [Signature], poi: &PoiEntry) -> Vec<(&'a Signature, u32)> {
    all.iter().filter_map(|sig| poi.jumps_from(sig.in_system_id).map(|d| (sig, d))).collect()
}

/// The point of interest the lower table lists: the one under the pointer, else the one chosen.
pub fn shown(s: &State) -> Option<&PoiEntry> {
    let id = s.poi_hovered.or(s.poi_selected)?;
    s.pois.iter().find(|p| p.id == id)
}

/// Rebuilds the lower table's rows for the point of interest shown.
pub fn refresh_rows(s: &mut State) {
    s.poi_rows = shown(s).map(|p| matches(&s.all, p).into_iter().map(|(sig, d)| (sig.clone(), d)).collect()).unwrap_or_default();
    let (col, dir) = s.poi_sort.clone();
    sort_rows(s, &col, dir);
}

fn sort_rows(s: &mut State, col: &str, dir: SortDir) {
    s.poi_sort = (col.to_string(), dir);
    s.poi_rows.sort_by(|(a, da), (b, db)| {
        let o = match col {
            "hub" => hub_name(a.hub()).cmp(hub_name(b.hub())),
            "system" => rune_kit::natural_cmp(&a.in_system_name, &b.in_system_name),
            "class" => a.in_system_class.cmp(&b.in_system_class),
            "region" => a.in_region_name.cmp(&b.in_region_name),
            "size" => a.size_rank().cmp(&b.size_rank()),
            "remaining" => a.expires_at.cmp(&b.expires_at),
            "age" => b.created_at.cmp(&a.created_at),
            _ => da.cmp(db),
        };
        dir.apply(o.then(da.cmp(db)).then(b.created_at.cmp(&a.created_at)))
    });
}

/// What the dialog is doing: adding a point of interest, or editing one (its system).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Editing {
    New,
    Existing(u64),
}

/// "Add point of interest": the dialog, empty.
pub fn open_add(s: &mut State) {
    s.poi_search = SystemSearch::default();
    s.poi_search.edit.focused = true;
    s.poi_pick = None;
    s.poi_kind = None;
    s.poi_comment = TextEdit::default();
    s.poi_jumps = DEFAULT_JUMPS;
    s.poi_dialog = Some(Editing::New);
}

/// A row's "Edit": the dialog, filled with the point of interest.
pub fn open_edit(s: &mut State, id: u64) {
    let Some(p) = s.pois.iter().find(|p| p.id == id) else { return };
    s.poi_search = SystemSearch::new(p.name());
    s.poi_pick = Some(id);
    s.poi_kind = p.kind;
    s.poi_comment = TextEdit::new(p.comment.clone());
    s.poi_comment.focused = true;
    s.poi_jumps = p.jumps;
    s.poi_dialog = Some(Editing::Existing(id));
}

pub fn close_dialog(s: &mut State) {
    s.poi_dialog = None;
    s.poi_search.open = false;
}

/// The dialog's "Add" or "Save": the point of interest is added (or replaced in place, when
/// editing it or adding a system listed already), shown, and the dialog closes.
pub fn submit(s: &mut State) {
    let (Some(id), Some(editing)) = (s.poi_pick, s.poi_dialog) else { return };
    let entry = PoiEntry::new(id, s.poi_kind, &s.poi_comment.text, s.poi_jumps);
    let at = match editing {
        Editing::Existing(old) => s.pois.iter().position(|p| p.id == old),
        Editing::New => s.pois.iter().position(|p| p.id == id),
    };
    match at {
        Some(i) => s.pois[i] = entry,
        None => s.pois.push(entry),
    }
    s.poi_selected = Some(id);
    close_dialog(s);
    refresh_rows(s);
}

pub fn remove(s: &mut State, id: u64) {
    s.pois.retain(|p| p.id != id);
    if s.poi_hovered == Some(id) {
        s.poi_hovered = None;
    }
    if s.poi_selected == Some(id) {
        s.poi_selected = s.pois.first().map(|p| p.id);
    }
    refresh_rows(s);
}

pub fn change_jumps(s: &mut State, id: u64, delta: i32) {
    if let Some(p) = s.pois.iter_mut().find(|p| p.id == id) {
        let jumps = p.jumps.saturating_add_signed(delta).min(MAX_JUMPS);
        if jumps != p.jumps {
            *p = PoiEntry::new(id, p.kind, &p.comment, jumps);
        }
    }
    refresh_rows(s);
}

fn select(s: &mut State, id: u64) {
    s.poi_selected = Some(id);
    refresh_rows(s);
}

fn hover(s: &mut State, id: u64, inside: bool) {
    if inside {
        s.poi_hovered = Some(id);
    } else if s.poi_hovered == Some(id) {
        s.poi_hovered = None;
    } else {
        return;
    }
    refresh_rows(s);
}

/// The points of interest within range of a connection's exit, closest first.
pub fn near_exit<'a>(s: &'a State, sig: &Signature) -> Vec<(&'a PoiEntry, u32)> {
    let mut found: Vec<(&PoiEntry, u32)> = s.pois.iter().filter_map(|p| p.jumps_from(sig.in_system_id).map(|d| (p, d))).collect();
    found.sort_by_key(|(p, d)| (*d, kind_rank(p.kind)));
    found
}

/// Which type matters most when several are near: an enemy staging first.
fn kind_rank(kind: Option<PoiKind>) -> u8 {
    match kind {
        Some(PoiKind::EnemyStaging) => 0,
        Some(PoiKind::Trading) => 1,
        Some(PoiKind::Other) => 2,
        None => 3,
    }
}

fn kind_color(k: &Kit, kind: Option<PoiKind>) -> Color {
    match kind {
        Some(PoiKind::EnemyStaging) => k.c.danger.icon,
        Some(PoiKind::Trading) => k.c.success.icon,
        Some(PoiKind::Other) => k.c.info.icon,
        None => k.c.icon_muted,
    }
}

/// The connections table's POI cell: a pin in the color of the most pressing type near the
/// exit, with how many, if several. Clicking it opens the details (`exit_card`).
pub fn exit_cell(s: &State, k: &Kit, sig: &Signature) -> Option<El> {
    let near = near_exit(s, sig);
    let top = near.iter().map(|(p, _)| p.kind).min_by_key(|kind| kind_rank(*kind))?;
    let id = sig.id.clone();
    let open = s.poi_hint.as_deref() == Some(id.as_str());
    let mut cell = row()
        .key(format!("poi-hint-{id}"))
        .role("button")
        .label(format!("{} point{} of interest nearby", near.len(), plural(near.len())))
        .h(22.0)
        .px(4.0)
        .gap(3.0)
        .center_y()
        .radius(4.0)
        .fill(if open { k.c.hover } else { Color::TRANSPARENT })
        .hover(k.c.hover)
        .cursor(Cursor::Pointer)
        .child(icon(&POI_ICON, 15.0).color(kind_color(k, top)).a11y_hidden());
    if near.len() > 1 {
        cell = cell.child(text(near.len().to_string()).font(k.ty.small_medium).color(k.c.text_muted).a11y_hidden());
    }
    Some(cell.on_press(move |s: &mut State| toggle_hint(s, &id)))
}

/// A click on a pin: opens its card, or closes it when it is the one open.
fn toggle_hint(s: &mut State, id: &str) {
    if s.poi_hint.as_deref() == Some(id) {
        close_hint(s);
    } else {
        close_hint(s);
        s.poi_hint = Some(id.to_string());
    }
}

/// Closes the card: its ✕, its pin, Esc, a click elsewhere.
pub fn close_hint(s: &mut State) {
    if route_card::holds_poi_card(s) {
        route_card::close(s);
    }
    s.poi_hint = None;
}

/// The details of the points of interest near the clicked exit, under its pin.
pub fn exit_card(s: &State, k: &Kit, layer: &mut Vec<El>) {
    let Some(sig) = s.poi_hint.as_ref().and_then(|id| s.all.iter().find(|x| &x.id == id)) else { return };
    let near = near_exit(s, sig);
    if near.is_empty() {
        return;
    }
    let u = universe::universe();
    let header = row()
        .w_full()
        .h(22.0)
        .gap(6.0)
        .center_y()
        .child(text(format!("Points of interest near {}", sig.in_system_name)).font(k.ty.ui_medium).color(k.c.text_base).truncate().shrink())
        .child(el().grow(1.0))
        .child(
            row()
                .key("poi-card-close")
                .role("button")
                .label("Close")
                .size(22.0, 22.0)
                .center()
                .radius(k.radius.sm)
                .hover(k.c.hover)
                .cursor(Cursor::Pointer)
                .child(icon(k.glyph.clear, 14.0).color(k.c.icon_muted))
                .on_press(close_hint),
        );
    let mut body = col().key("poi-card-body").w(340.0).padding(10.0).gap(8.0).child(header);
    for (p, d) in near {
        let sys = u.system(p.id);
        let head = row()
            .w_full()
            .gap(8.0)
            .center_y()
            .child(kind_pill(k, p.kind))
            .child(text(p.name()).font(k.ty.ui_medium).color(k.c.text_base))
            .child(match sys {
                Some(x) => text(format!("{:.1}", x.display_sec())).font(k.ty.mono).color(space_color(k, x.space())),
                None => el(),
            })
            .child(el().grow(1.0))
            .child(route_card::jumps(
                s,
                k,
                format!("route-card-{}", p.id),
                sig.in_system_id,
                p.id,
                jumps_label(d),
                k.c.text_accent,
            ));
        let mut item = col().w_full().gap(3.0).child(head);
        let region = sys.map_or("", |x| x.region.as_str());
        let mut detail = format!("{region} · within {} jumps", p.jumps);
        if !p.comment.is_empty() {
            detail = format!("{} · {detail}", p.comment);
        }
        item = item.child(text(detail).font(k.ty.small).color(k.c.text_muted).wrap());
        body = body.child(item);
    }
    let anchor = format!("poi-hint-{}", sig.id);
    // A click anywhere else closes it (and only that).
    layer.push(rune_kit::scrim("poi-card-scrim", close_hint));
    let card = rune_kit::popover::raised(k, body, k.radius.md)
        .key("poi-card")
        // A hit target, so the rows under it neither light up nor take its clicks.
        .on_press(|_: &mut State| {});
    layer.push(rune_kit::popover::anchored(
        k,
        card,
        &anchor,
        Placement::BottomStart,
        4.0,
    ));
}

// ---------------------------------------------------------------------------------------------
// View

const ROW_H: f32 = 40.0;
const HEADER_H: f32 = 30.0;

/// The page's columns: (title, width or flex share as a negative number).
const COLUMNS: [(&str, f32); 8] = [
    ("Type", 130.0),
    ("Comment", -1.2),
    ("System", -1.0),
    ("Region", 140.0),
    ("Max jumps", 120.0),
    ("Connections", 110.0),
    ("Closest connection", -1.6),
    ("", 140.0),
];

fn sized(e: El, i: usize) -> El {
    let w = COLUMNS[i].1;
    let e = e.px(10.0).center_y().min_w(0.0);
    if w < 0.0 { e.grow(-w).w(0.0) } else { e.w(w) }
}

pub fn page(s: &State, k: &Kit) -> El {
    let frame = |key: &str, child: El| {
        col()
            .key(key)
            .w_full()
            .min_h(0.0)
            .radius(k.radius.md)
            .fill(k.c.bg_base)
            .clip_rounded(k.radius.md)
            .child(child)
    };
    // The splitter divides this area between the points of interest and their connections.
    let split = col()
        .key("poi-split")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .on_layout(|s: &mut State, r| {
            if s.poi_split_area_h != Some(r[3]) {
                s.poi_split_area_h = Some(r[3]);
            }
        })
        .child(frame("poi-list", poi_list(s, k)).h(list_height(s)))
        .child(splitter(k, "poi-splitter", "Resize the points of interest and their connections", drag_splitter, |s| s.poi_list_h = None))
        .child(
            col()
                .key("poi-bottom")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .gap(10.0)
                .child(matches_header(s, k))
                .child(frame("poi-matches", matches_table(s, k)).grow(1.0)),
        );
    col()
        .key("poi-page")
        .w_full()
        .h_full()
        .padding(12.0)
        .gap(10.0)
        .child(status_line(s, k))
        .child(split)
}

// Splitter between the points of interest and their connections

/// The list's height until dragged: its rows, up to six.
fn fit_height(s: &State) -> f32 {
    HEADER_H + ROW_H * s.pois.len().clamp(1, 6) as f32
}

/// The list keeps its header and a row; the connections keep their header and a few rows.
const MIN_LIST_H: f32 = HEADER_H + ROW_H;
const MIN_MATCHES_H: f32 = 150.0;

fn max_list(s: &State) -> f32 {
    s.poi_split_area_h.map_or(f32::MAX, |h| (h - SPLITTER_H - MIN_MATCHES_H).max(MIN_LIST_H))
}

/// The list's height as drawn: what the splitter set (or its rows), within what the window allows.
pub(super) fn list_height(s: &State) -> f32 {
    s.poi_list_h.unwrap_or_else(|| fit_height(s)).clamp(MIN_LIST_H, max_list(s))
}

fn drag_splitter(s: &mut State, d: &mut Drag) {
    match d.phase {
        // Remember the height at the press; moves are relative to it.
        Phase::Down => d.data = list_height(s),
        Phase::Move | Phase::Up => s.poi_list_h = Some((d.data + d.dy).clamp(MIN_LIST_H, max_list(s))),
    }
}

fn status_line(s: &State, k: &Kit) -> El {
    let n = s.pois.len();
    let line = match (&s.error, s.last_update) {
        (Some(e), _) => text(format!("Error: {e}")).color(k.c.danger.icon),
        (None, None) => text("Loading EVE-Scout data…").color(k.c.text_muted),
        (None, Some(t)) => text(format!(
            "{n} point{} of interest · {} Thera and Turnur connections · updated {} ago",
            plural(n),
            s.all.len(),
            ago(s.now, t)
        ))
        .color(k.c.text_muted),
    };
    row()
        .key("poi-status")
        .w_full()
        .center_y()
        .gap(10.0)
        .child(line.font(k.ty.text_small).truncate().shrink().min_w(0.0))
        .child(el().grow(1.0))
        .child(add_button(k))
}

static PLUS: Icon = Icon {
    name: "poi-plus",
    view: 16.0,
    body: r#"<path d="M8 3V13M3 8H13" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>"#,
};

/// The green "+ Add point of interest" button, at the page's top right.
fn add_button(k: &Kit) -> El {
    // A solid green (#238636) that keeps white text readable in both themes.
    let green = rune::oklch(0.56, 0.14, 146.0);
    let white = Color::WHITE;
    row()
        .key("poi-open-add")
        .role("button")
        .label("Add point of interest")
        .h(30.0)
        .px(12.0)
        .gap(6.0)
        .center()
        .radius(k.radius.md)
        .fill(green)
        .hover(green.mix(white, 0.12))
        .press(green.mix(Color::BLACK, 0.15))
        .cursor(Cursor::Pointer)
        .child(icon(&PLUS, 14.0).color(white).a11y_hidden())
        .child(text("Add point of interest").font(k.ty.ui_medium).color(white).a11y_hidden())
        .on_press(open_add)
}

/// A row of − value + to change a number of jumps.
fn stepper(k: &Kit, value: u32, key: String, f: impl Fn(&mut State, i32) + Clone + 'static) -> El {
    let (minus, plus) = (f.clone(), f);
    row()
        .key(key)
        .gap(2.0)
        .center_y()
        .child(
            Button::new("−")
                .variant(ButtonVariant::Ghost)
                .disabled(value == 0)
                .on_press(move |s: &mut State| minus(s, -1))
                .build(k)
                .key("less"),
        )
        .child(text(value.to_string()).font(k.ty.mono).color(k.c.text_base).w(26.0).align(Align::Center))
        .child(
            Button::new("+")
                .variant(ButtonVariant::Ghost)
                .disabled(value >= MAX_JUMPS)
                .on_press(move |s: &mut State| plus(s, 1))
                .build(k)
                .key("more"),
        )
}

const KIND_LABELS: [&str; 4] = ["None", "Enemy staging", "Trading system", "Other"];

fn kind_index(kind: Option<PoiKind>) -> usize {
    kind.map_or(0, |x| 1 + PoiKind::ALL.iter().position(|&k| k == x).unwrap_or(0))
}

/// The add / edit dialog: system, type, comment and jumps, one per line.
pub fn dialog(s: &State, k: &Kit, layer: &mut Vec<El>) {
    let Some(editing) = s.poi_dialog else { return };
    let line = |title: &str, hint: &str, control: El| {
        col()
            .w_full()
            .gap(6.0)
            .py(6.0)
            .child(
                row()
                    .w_full()
                    .gap(8.0)
                    .center_y()
                    .child(text(title.to_string()).font(k.ty.ui_medium).color(k.c.text_base))
                    .child(text(hint.to_string()).font(k.ty.small).color(k.c.text_muted).truncate().shrink().min_w(0.0)),
            )
            .child(control)
    };
    let system = match editing {
        Editing::New => system_field(s, k, Field::Poi, "Search a system…").w_full(),
        Editing::Existing(id) => {
            let u = universe::universe();
            let sys = u.system(id);
            row()
                .h(32.0)
                .gap(8.0)
                .center_y()
                .child(text(system_name(id).unwrap_or("?")).font(k.ty.ui_medium).color(k.c.text_base))
                .child(match sys {
                    Some(x) => text(format!("{:.1}", x.display_sec())).font(k.ty.mono).color(space_color(k, x.space())),
                    None => el(),
                })
                .child(text(sys.map_or("", |x| x.region.as_str())).font(k.ty.text_small).color(k.c.text_muted))
        }
    };
    let kind = SegmentedControl::new(&KIND_LABELS, Some(kind_index(s.poi_kind)))
        .label("Type")
        .fit()
        .on_pick(|s: &mut State, i, _| s.poi_kind = i.checked_sub(1).and_then(|i| PoiKind::ALL.get(i).copied()))
        .build(k)
        .key("poi-kind");
    let comment = TextInput::new(&s.poi_comment)
        .placeholder("Their staging, cheap ammo…")
        .label("Comment")
        .width(None)
        .on_edit(|s: &mut State| &mut s.poi_comment)
        .on_submit(|s: &mut State, _| submit(s))
        .build(k)
        .key("poi-comment");
    let jumps = row()
        .gap(10.0)
        .center_y()
        .child(stepper(k, s.poi_jumps, "poi-form-jumps".into(), |s: &mut State, d| {
            s.poi_jumps = s.poi_jumps.saturating_add_signed(d).min(MAX_JUMPS);
        }))
        .child(text("gate jumps").font(k.ty.text_small).color(k.c.text_muted));
    let listed = matches!(editing, Editing::New) && s.poi_pick.is_some_and(|id| s.pois.iter().any(|p| p.id == id));
    let label = match editing {
        Editing::Existing(_) => "Save",
        Editing::New if listed => "Update",
        Editing::New => "Add",
    };
    let buttons = row()
        .w_full()
        .gap(8.0)
        .pt(10.0)
        .center_y()
        .child(if listed {
            text("Already listed: its type, comment and jumps will be replaced").font(k.ty.small).color(k.c.text_muted)
        } else {
            el()
        })
        .child(el().grow(1.0))
        .child(Button::new("Cancel").variant(ButtonVariant::Neutral).on_press(close_dialog).build(k).key("poi-cancel"))
        .child(
            Button::new(label)
                .variant(ButtonVariant::Submit)
                .disabled(s.poi_pick.is_none())
                .on_press(submit)
                .build(k)
                .key("poi-submit"),
        );
    let body = col()
        .key("poi-dialog-body")
        .w_full()
        .px(16.0)
        .pb(16.0)
        .child(line("System", if editing == Editing::New { "type three letters" } else { "" }, system))
        .child(line("Type", "optional", kind))
        .child(line("Comment", "optional", comment))
        .child(line("Max jumps", "lists the Thera and Turnur exits within this many jumps", jumps))
        .child(buttons);
    let title = match editing {
        Editing::New => "Add a point of interest".to_string(),
        Editing::Existing(id) => format!("Edit {}", system_name(id).unwrap_or("point of interest")),
    };
    Dialog::new("poi-dialog", title)
        .open(true)
        .width(520.0)
        .fit()
        .on_close(close_dialog)
        .child(body)
        .build(k, layer);
}

/// The type as a colored pill.
pub(super) fn kind_pill(k: &Kit, kind: Option<PoiKind>) -> El {
    let Some(kind) = kind else {
        return text("—").color(k.c.text_faint);
    };
    let c = match kind {
        PoiKind::EnemyStaging => k.c.danger,
        PoiKind::Trading => k.c.success,
        PoiKind::Other => k.c.info,
    };
    row()
        .h(20.0)
        .px(7.0)
        .center_y()
        .radius(4.0)
        .fill(c.bg)
        .child(text(kind.label()).font(k.ty.text_small).color(c.fg))
}

fn poi_list(s: &State, k: &Kit) -> El {
    let style = rune_kit::TableStyle::from_kit(k);
    let mut header = row().key("poi-header").w_full().h(HEADER_H).center_y();
    for (i, (title, _)) in COLUMNS.iter().enumerate() {
        header = header.child(sized(row().child(text(*title).font(k.ty.text_small).color(k.c.text_muted)), i));
    }
    let mut rows = col().key("poi-rows").w_full();
    if s.pois.is_empty() {
        rows = rows.child(
            row()
                .h(ROW_H)
                .px(10.0)
                .center_y()
                .child(text("No point of interest yet: \"Add point of interest\" above").font(k.ty.text_small).color(k.c.text_muted)),
        );
    }
    for (i, p) in s.pois.iter().enumerate() {
        rows = rows.child(poi_row(s, k, p, i, &style));
    }
    let viewport = rows.scroll(rune::ScrollSpec { stick_bottom: false }).w_full().grow(1.0).min_h(0.0);
    col()
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(header)
        .child(el().w_full().h(1.0).fill(k.c.border_muted))
        .child(rune_kit::scroll_bar(k, "poi-area", "poi-scroll", viewport).w_full().grow(1.0).min_h(0.0))
}

fn poi_row(s: &State, k: &Kit, p: &PoiEntry, i: usize, style: &rune_kit::TableStyle) -> El {
    let id = p.id;
    let u = universe::universe();
    let sys = u.system(id);
    let found = matches(&s.all, p);
    let closest = found.iter().min_by_key(|(sig, d)| (*d, std::cmp::Reverse(sig.created_at)));
    let name = row()
        .gap(8.0)
        .center_y()
        .child(text(p.name()).font(k.ty.ui_medium).color(k.c.text_base).truncate().shrink())
        .child(match sys {
            Some(x) => text(format!("{:.1}", x.display_sec())).font(k.ty.mono).color(space_color(k, x.space())),
            None => el(),
        });
    let region = text(sys.map_or("", |x| x.region.as_str())).font(k.ty.text_small).color(k.c.text_muted).truncate();
    let jumps = stepper(k, p.jumps, format!("poi-jumps-{id}"), move |s: &mut State, d| change_jumps(s, id, d));
    let count = match found.len() {
        0 => text("none").font(k.ty.text_small).color(k.c.text_faint),
        n => text(format!("{n} connection{}", plural(n))).font(k.ty.text_small).color(k.c.text_accent),
    };
    let closest = match closest {
        Some((sig, d)) => text(format!("{} › {} · {}", hub_name(sig.hub()), sig.in_system_name, jumps_label(*d)))
            .font(k.ty.text_small)
            .color(k.c.text_base)
            .truncate(),
        None => text("—").color(k.c.text_faint),
    };
    let remove_button = row()
        .gap(2.0)
        .center_y()
        .child(Button::new("Edit").variant(ButtonVariant::Ghost).on_press(move |s: &mut State| open_edit(s, id)).build(k).key("edit"))
        .child(Button::new("Remove").variant(ButtonVariant::Ghost).on_press(move |s: &mut State| remove(s, id)).build(k).key("remove"));
    let selected = s.poi_selected == Some(id);
    let hovered = s.poi_hovered == Some(id);
    let fill = if selected {
        style.selected
    } else if hovered {
        k.c.hover
    } else if i % 2 == 1 {
        style.stripe
    } else {
        Color::TRANSPARENT
    };
    let comment = text(p.comment.clone()).font(k.ty.text_small).color(k.c.text_muted).truncate().help_when_truncated(p.comment.clone());
    let cells = [kind_pill(k, p.kind), comment, name, region, jumps, count, closest, remove_button];
    cells
        .into_iter()
        .enumerate()
        .fold(row().key(format!("poi-{id}")).role("row").w_full().h(ROW_H).center_y().fill(fill), |r, (i, c)| {
            r.child(sized(row().h_full().child(c), i))
        })
        .on_hover_change(move |s: &mut State, inside, _| hover(s, id, inside))
        .on_press(move |s: &mut State| select(s, id))
}

fn matches_header(s: &State, k: &Kit) -> El {
    let (title, info) = match shown(s) {
        Some(p) => {
            let note: Vec<&str> = p.kind.map(PoiKind::label).into_iter().chain(Some(p.comment.as_str()).filter(|c| !c.is_empty())).collect();
            let kind = if note.is_empty() { String::new() } else { format!(" ({})", note.join(" · ")) };
            let n = s.poi_rows.len();
            (
                format!("Connections ≤{} jumps from {}{kind}", p.jumps, p.name()),
                format!(
                    "{n} of {} connection{} · hover a point of interest to compare, click one to keep it",
                    s.all.len(),
                    plural(s.all.len())
                ),
            )
        }
        None => ("Connections near a point of interest".into(), "Hover or click a point of interest".into()),
    };
    row()
        .key("poi-matches-header")
        .w_full()
        .h(32.0)
        .min_h(32.0)
        .center_y()
        .gap(12.0)
        .child(text(title).font(k.ty.body_medium).color(k.c.text_base))
        .child(text(info).font(k.ty.text_small).color(k.c.text_muted).truncate().min_w(0.0).shrink())
}

fn matches_table(s: &State, k: &Kit) -> El {
    let ck = k.clone();
    let closest = s.poi_rows.iter().map(|(_, d)| *d).min();
    Table::new(
        |s: &State| &s.poi_table,
        |s: &mut State| &mut s.poi_table,
        |s: &State| s.poi_rows.len(),
        |s: &State, i| Cow::Borrowed(s.poi_rows[i].0.id.as_str()),
    )
    .name("Connections near the point of interest")
    .column(TableColumn::new("hub", "Hub").width(80.0).sortable())
    .column(TableColumn::new("system", "Exit").flex(1.0).min(96.0).sortable())
    .column(TableColumn::new("jumps", "Jumps").width(80.0).align(Align::End).sortable())
    .column(TableColumn::new("class", "Class").width(56.0).sortable())
    .column(TableColumn::new("region", "Region").flex(0.8).min(90.0).sortable())
    .column(TableColumn::new("hub_sig", "Hub sig").width(96.0))
    .column(TableColumn::new("in_sig", "Exit sig").width(84.0))
    .column(TableColumn::new("size", "Max size").width(82.0).sortable())
    .column(TableColumn::new("remaining", "Remaining").width(94.0).align(Align::End).sortable())
    .column(TableColumn::new("age", "Age").width(72.0).align(Align::End).sortable())
    .cell(move |s: &State, i, column| {
        let (sig, d) = &s.poi_rows[i];
        let k = &ck;
        match column {
            "hub" => Cell::text(hub_name(sig.hub())),
            "system" => Cell::text(&sig.in_system_name),
            "jumps" => {
                let color = if Some(*d) == closest { k.c.text_accent } else { k.c.text_base };
                match shown(s) {
                    Some(p) => Cell::el(route_card::jumps(s, k, format!("route-poim-{}", sig.id), sig.in_system_id, p.id, d.to_string(), color)),
                    None => Cell::text(d.to_string()).mono(),
                }
            }
            "class" => Cell::text(sig.class_label()).color(space_color(k, sig.space())),
            "region" => Cell::text(&sig.in_region_name).muted(),
            "hub_sig" => Cell::el(hub_sig_pill(k, &sig.out_signature)),
            "in_sig" => Cell::text(&sig.in_signature).mono(),
            "size" => Cell::text(sig.size_label()),
            "remaining" => {
                let left = sig.expires_at - s.now;
                let c = Cell::text(duration(left)).mono();
                match remaining_color(k, left) {
                    Some(color) => c.color(color),
                    None => c,
                }
            }
            "age" => Cell::text(duration(s.now - sig.created_at)).mono().muted(),
            _ => Cell::text(""),
        }
    })
    .on_sort(|s: &mut State, col, dir, _| sort_rows(s, col, dir))
    .on_open(|s: &mut State, id, tx| {
        if let Some(name) = s.poi_rows.iter().find(|(r, _)| r.id == id).map(|(r, _)| r.in_system_name.clone()) {
            tx.copy(name);
        }
    })
    .zebra(true)
    .build(s, k)
    .grow(1.0)
    .min_h(0.0)
}

#[cfg(test)]
mod tests {
    use super::super::tests::loaded;
    use super::*;

    #[test]
    fn clicking_a_pin_opens_its_card_and_clicking_again_closes_it() {
        let mut s = loaded();
        let (a, b) = (s.all[0].id.clone(), s.all[1].id.clone());
        toggle_hint(&mut s, &a);
        assert_eq!(s.poi_hint.as_deref(), Some(a.as_str()));
        toggle_hint(&mut s, &b);
        assert_eq!(s.poi_hint.as_deref(), Some(b.as_str()), "another pin takes over");
        toggle_hint(&mut s, &b);
        assert_eq!(s.poi_hint, None);
    }

    #[test]
    fn jita_is_there_from_the_start() {
        let s = init();
        assert_eq!(s.pois.len(), 1);
        assert_eq!(s.pois[0].name(), "Jita");
        assert_eq!(s.pois[0].kind, Some(PoiKind::Trading));
        assert_eq!(s.poi_selected, Some(s.pois[0].id));
    }

    #[test]
    fn the_listed_connections_are_within_the_jumps() {
        let mut s = loaded();
        let jita = s.pois[0].id;
        select(&mut s, jita);
        let all = matches(&s.all, &s.pois[0]).len();
        assert!(all > 0, "the fixtures have exits near Jita");
        assert_eq!(s.poi_rows.len(), all);
        assert!(s.poi_rows.iter().all(|(_, d)| *d <= 10));
        assert!(s.poi_rows.windows(2).all(|w| w[0].1 <= w[1].1), "closest first");
        let u = universe::universe();
        for (sig, d) in &s.poi_rows {
            assert_eq!(u.distances(jita).get(&sig.in_system_id), Some(d));
        }
        change_jumps(&mut s, jita, -5);
        assert!(s.poi_rows.len() <= all && s.poi_rows.iter().all(|(_, d)| *d <= 5));
    }

    #[test]
    fn hovering_shows_another_point_of_interest_until_the_pointer_leaves() {
        let mut s = loaded();
        let jita = s.pois[0].id;
        let amarr = universe::universe().find("Amarr").unwrap();
        open_add(&mut s);
        search_pick(&mut s, Field::Poi, amarr);
        s.poi_kind = Some(PoiKind::EnemyStaging);
        s.poi_comment = TextEdit::new("  Their home  ");
        s.poi_jumps = 5;
        submit(&mut s);
        assert_eq!(s.poi_dialog, None, "the dialog closes");
        assert_eq!(s.pois.len(), 2);
        assert_eq!(s.pois[1].comment, "Their home");
        open_add(&mut s);
        assert_eq!((s.poi_pick, s.poi_search.edit.text.as_str(), s.poi_comment.text.as_str()), (None, "", ""), "it opens empty");
        close_dialog(&mut s);
        select(&mut s, jita);
        hover(&mut s, amarr, true);
        assert_eq!(shown(&s).map(|p| p.id), Some(amarr));
        // Entering a row before leaving the other keeps the new one.
        hover(&mut s, jita, true);
        hover(&mut s, amarr, false);
        assert_eq!(shown(&s).map(|p| p.id), Some(jita));
        hover(&mut s, jita, false);
        assert_eq!(shown(&s).map(|p| p.id), Some(jita), "back to the one clicked");
    }

    #[test]
    fn adding_a_listed_system_updates_it_and_removing_moves_the_selection() {
        let mut s = loaded();
        let jita = s.pois[0].id;
        open_add(&mut s);
        search_pick(&mut s, Field::Poi, jita);
        s.poi_kind = None;
        s.poi_jumps = 3;
        submit(&mut s);
        assert_eq!(s.pois.len(), 1);
        assert_eq!((s.pois[0].kind, s.pois[0].jumps), (None, 3));
        remove(&mut s, jita);
        assert!(s.pois.is_empty() && s.poi_selected.is_none() && s.poi_rows.is_empty());
        assert!(save(&s.pois).is_empty());
    }

    #[test]
    fn editing_fills_the_dialog_and_saves_in_place() {
        let mut s = loaded();
        let amarr = universe::universe().find("Amarr").unwrap();
        s.pois.push(PoiEntry::new(amarr, Some(PoiKind::EnemyStaging), "old note", 5));
        let jita = s.pois[0].id;
        open_edit(&mut s, amarr);
        assert_eq!(s.poi_dialog, Some(Editing::Existing(amarr)));
        assert_eq!((s.poi_pick, s.poi_kind, s.poi_comment.text.as_str(), s.poi_jumps), (Some(amarr), Some(PoiKind::EnemyStaging), "old note", 5));
        s.poi_comment = TextEdit::new("new note");
        s.poi_jumps = 7;
        submit(&mut s);
        assert_eq!(s.pois.iter().map(|p| p.id).collect::<Vec<_>>(), vec![jita, amarr], "same place in the list");
        assert_eq!((s.pois[1].comment.as_str(), s.pois[1].jumps), ("new note", 7));
        // Cancel changes nothing.
        open_edit(&mut s, amarr);
        s.poi_jumps = 1;
        close_dialog(&mut s);
        assert_eq!(s.pois[1].jumps, 7);
    }

    #[test]
    fn nothing_is_added_without_a_system() {
        let mut s = loaded();
        open_add(&mut s);
        submit(&mut s);
        assert_eq!(s.pois.len(), 1);
        assert_eq!(s.poi_dialog, Some(Editing::New), "the dialog stays open");
    }

    #[test]
    fn points_of_interest_round_trip_through_the_settings() {
        let saved = vec![
            Poi { system: "amarr".into(), kind: Some(PoiKind::Other), comment: "ammo".into(), jumps: 7 },
            Poi { system: "Nowhere".into(), kind: None, comment: String::new(), jumps: 3 },
            Poi { system: "Amarr".into(), kind: None, comment: String::new(), jumps: 2 },
        ];
        let list = load(&saved);
        assert_eq!(save(&list), vec![Poi { system: "Amarr".into(), kind: Some(PoiKind::Other), comment: "ammo".into(), jumps: 7 }]);
    }
}
