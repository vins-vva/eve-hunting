//! The route behind a number of jumps: clicking one shows a card listing the systems on the
//! way, each with its security, from the connection's exit to the system the number counts to.
//! Clicking the number again, anywhere else, or Esc closes it.

use super::*;

const ROW_H: f32 = 22.0;
/// The most rows shown before the route scrolls.
const MAX_ROWS: usize = 16;
const WIDTH: f32 = 300.0;

/// An open route card: the number it hangs from (its element key) and the route.
pub struct RouteHint {
    key: String,
    path: Vec<u64>,
}

/// A number of jumps that shows its route when clicked: `from` the connection's exit `to` the
/// system counted to. `key` names it (unique on the page); `label` is what it reads.
pub fn jumps(s: &State, k: &Kit, key: String, from: u64, to: u64, label: String, color: Color) -> El {
    let open = s.route_hint.as_ref().is_some_and(|h| h.key == key);
    let press_key = key.clone();
    row()
        .key(key)
        .role("button")
        .label(format!("{label}: show the route"))
        .h(20.0)
        .px(4.0)
        .mx(-4.0)
        .center_y()
        .radius(4.0)
        .fill(if open { k.c.hover } else { Color::TRANSPARENT })
        .hover(k.c.hover)
        .cursor(Cursor::Pointer)
        .child(text(label).font(k.ty.mono).color(color).a11y_hidden())
        .on_press(move |s: &mut State| toggle(s, &press_key, from, to))
}

/// A click on a number: opens its route, or closes it when it is the one open.
fn toggle(s: &mut State, key: &str, from: u64, to: u64) {
    if s.route_hint.as_ref().is_some_and(|h| h.key == key) {
        s.route_hint = None;
    } else {
        s.route_hint = universe::universe().path(from, to).map(|path| RouteHint { key: key.to_string(), path });
    }
}

/// Whether a route card hangs from inside the POI card, which must then stay open.
pub fn holds_poi_card(s: &State) -> bool {
    s.route_hint.as_ref().is_some_and(|h| h.key.starts_with("route-card-"))
}

pub fn close(s: &mut State) {
    s.route_hint = None;
}

/// The open route card, under its number.
pub fn card(s: &State, k: &Kit, layer: &mut Vec<El>) {
    let Some(h) = &s.route_hint else { return };
    let u = universe::universe();
    let (Some(&first), Some(&last)) = (h.path.first(), h.path.last()) else { return };
    let n = h.path.len() - 1;
    let header = row()
        .w_full()
        .gap(8.0)
        .center_y()
        .child(
            text(format!("{} → {}", system_name(first).unwrap_or("?"), system_name(last).unwrap_or("?")))
                .font(k.ty.ui_medium)
                .color(k.c.text_base)
                .truncate()
                .shrink(),
        )
        .child(el().grow(1.0))
        .child(text(jumps_label(n as u32)).font(k.ty.mono).color(k.c.text_accent));
    let mut rows = col().key("route-rows").w_full();
    let mut region = "";
    for (i, &id) in h.path.iter().enumerate() {
        let Some(sys) = u.system(id) else { continue };
        // The region shows where it changes, so a long route reads as its regions.
        let region_cell = if sys.region != region {
            region = sys.region.as_str();
            text(region).font(k.ty.small).color(k.c.text_muted).truncate()
        } else {
            el()
        };
        let ends = i == 0 || i == n;
        rows = rows.child(
            row()
                .w_full()
                .h(ROW_H)
                .gap(8.0)
                .center_y()
                .child(text(i.to_string()).font(k.ty.mono).color(k.c.text_faint).w(22.0).align(Align::End))
                .child(
                    text(sys.name.clone())
                        .font(if ends { k.ty.ui_medium } else { k.ty.ui })
                        .color(k.c.text_base)
                        .w(110.0)
                        .truncate(),
                )
                .child(text(format!("{:.1}", sys.display_sec())).font(k.ty.mono).color(space_color(k, sys.space())).w(34.0))
                .child(region_cell.grow(1.0).min_w(0.0)),
        );
    }
    let height = h.path.len().min(MAX_ROWS) as f32 * ROW_H;
    let viewport = rows.scroll(rune::ScrollSpec { stick_bottom: false }).w_full().h(height);
    let body = col()
        .key("route-card-body")
        .w(WIDTH)
        .padding(10.0)
        .gap(6.0)
        .child(header)
        .child(el().w_full().h(1.0).fill(k.c.border_muted))
        .child(rune_kit::scroll_bar(k, "route-area", "route-scroll", viewport).w_full().h(height));
    let card = rune_kit::popover::raised(k, body, k.radius.md)
        .key("route-card")
        // A hit target, so what is under it neither lights up nor takes its clicks.
        .on_press(|_: &mut State| {});
    // A click anywhere else closes it (and only that).
    layer.push(rune_kit::scrim("route-card-scrim", close));
    layer.push(rune_kit::popover::anchored(k, card, &h.key, Placement::BottomEnd, 4.0));
}

#[cfg(test)]
mod tests {
    use super::super::tests::loaded;
    use super::*;

    #[test]
    fn clicking_a_number_opens_its_route_and_clicking_again_closes_it() {
        let mut s = loaded();
        let u = universe::universe();
        let (jita, amarr) = (u.find("Jita").unwrap(), u.find("Amarr").unwrap());
        toggle(&mut s, "route-x", jita, amarr);
        let path = &s.route_hint.as_ref().unwrap().path;
        assert_eq!((path[0], *path.last().unwrap()), (jita, amarr));
        toggle(&mut s, "route-y", amarr, jita);
        assert_eq!(s.route_hint.as_ref().unwrap().path[0], amarr, "another number takes over");
        toggle(&mut s, "route-y", amarr, jita);
        assert!(s.route_hint.is_none());
    }
}
