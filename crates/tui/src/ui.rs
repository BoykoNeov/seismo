//! The screen: a header with the clock, the map or the slice above the traces on the left, the
//! event list and the selected event's details on the right, and a line of key hints.

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, View};
use crate::canvas::Canvas;
use crate::guide::Page;
use crate::map::MapScene;
use crate::palette::{Ink, Palette};
use crate::seismograms::{self, TraceScene};
use crate::slice::{self, SliceScene};

/// Smallest terminal drawn.
pub const MIN_SIZE: (u16, u16) = (60, 16);
/// The side panel shows from this terminal width.
const SIDE_FROM_WIDTH: u16 = 100;
/// Rows the traces panel gets at least, when shown.
const TRACES_MIN_ROWS: u16 = 7;

pub fn draw(f: &mut Frame, app: &mut App) {
    let pal = Palette::new(app.look);
    let area = f.area();
    let base = Style::default()
        .fg(pal.colour(Ink::Text))
        .bg(pal.background());
    f.render_widget(Block::default().style(base), area);
    if area.width < MIN_SIZE.0 || area.height < MIN_SIZE.1 {
        let msg = format!(
            "SEISMO needs a terminal of at least {}×{} (this one is {}×{}).",
            MIN_SIZE.0, MIN_SIZE.1, area.width, area.height
        );
        f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }), area);
        app.map_area = None;
        return;
    }
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(header_line(app, &pal)), header);
    f.render_widget(Paragraph::new(footer_line(&pal)), footer);

    let (main, side) = if area.width >= SIDE_FROM_WIDTH {
        let side_w = (area.width * 3 / 10).clamp(32, 56);
        let [m, s] =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(side_w)]).areas(body);
        (m, Some(s))
    } else {
        (body, None)
    };

    // The map and the slice both want a width of four times their height (square dots,
    // 360° by 180°, and a half-disc twice as wide as high).
    let traces_rows = if app.traces.show { TRACES_MIN_ROWS } else { 0 };
    let view_h = main
        .height
        .saturating_sub(traces_rows)
        .min(main.width.div_ceil(4))
        .max(4);
    let [view_area, trace_area] =
        Layout::vertical([Constraint::Length(view_h), Constraint::Min(0)]).areas(main);
    draw_view(f, app, &pal, view_area);
    if app.traces.show && trace_area.height >= 3 {
        draw_traces(f, app, &pal, trace_area);
    }
    if let Some(side) = side {
        draw_side(f, app, &pal, side);
    }
    if app.help {
        match app.guide.page {
            Page::Keys => draw_help(f, &pal, area),
            Page::Topics | Page::Topic(_) => draw_guide(f, app, &pal, area),
        }
    }
}

fn draw_view(f: &mut Frame, app: &mut App, pal: &Palette, area: Rect) {
    let w = area.width.min(4 * area.height);
    let h = area.height.min(w.div_ceil(4)).max(1);
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y,
        width: w,
        height: h,
    };
    let mut canvas = Canvas::new(w as usize, h as usize, *pal);
    let sel = app.sel();
    match app.view {
        View::Map => {
            let scene = MapScene {
                sim: &app.sim,
                now: app.clock.now(),
                selected: sel,
                stations: &app.traces.stations,
                chosen: &app.traces.chosen,
                // Where the slice cuts, so [ ] can aim it from the map.
                slice_azimuth: Some(app.azimuth_deg),
            };
            app.map.draw(&mut canvas, &scene);
            app.map_area = Some(rect);
        }
        View::Slice => {
            let e = &app.sim.events[sel];
            let (depth, t) = (e.depth_km, e.t_at(app.clock.now()));
            let fans = app.slice.fans(depth);
            slice::draw(
                &mut canvas,
                &SliceScene {
                    fans,
                    boundaries: app.sim.boundaries(),
                    depth_km: depth,
                    t,
                    azimuth_deg: app.azimuth_deg,
                },
            );
            app.map_area = None;
        }
    }
    canvas.render(rect, f.buffer_mut());
}

fn draw_traces(f: &mut Frame, app: &App, pal: &Palette, area: Rect) {
    let shown = app.traces.shown();
    let (from_s, to_s) = app.traces.window();
    let mut canvas = Canvas::new(area.width as usize, area.height as usize, *pal);
    seismograms::draw(
        &mut canvas,
        &TraceScene {
            traces: &shown,
            stations: &app.traces.stations,
            component: app.traces.component,
            shared_scale: app.traces.shared_scale,
            from_s,
            to_s,
            t: app.t(),
            pending: app.traces.pending,
        },
    );
    canvas.render(area, f.buffer_mut());
}

fn style(pal: &Palette, ink: Ink) -> Style {
    Style::default().fg(pal.colour(ink))
}

/// How long ago `origin` was at `now`, briefly.
pub fn age(origin: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let s = (now - origin).num_seconds();
    let a = s.abs();
    let span = if a < 60 {
        format!("{a}s")
    } else if a < 3600 {
        format!("{}m", a / 60)
    } else if a < 48 * 3600 {
        format!("{}h", a / 3600)
    } else if a < 300 * 86_400 {
        format!("{}d", a / 86_400)
    } else {
        return origin.format("%Y-%m-%d").to_string();
    };
    if s < 0 {
        format!("in {span}")
    } else {
        format!("{span} ago")
    }
}

/// Seconds as "+1h 02m 03s" / "-45s".
pub fn signed_duration(t: f64) -> String {
    let sign = if t < 0.0 { '-' } else { '+' };
    let s = t.abs().floor() as i64;
    let (h, m, sec) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{sign}{h}h {m:02}m {sec:02}s")
    } else if m > 0 {
        format!("{sign}{m}m {sec:02}s")
    } else {
        format!("{sign}{sec}s")
    }
}

fn header_line(app: &App, pal: &Palette) -> Line<'static> {
    let now = app.clock.now();
    let play = if app.clock.playing() { "▶" } else { "‖" };
    let speed = app.clock.speed();
    let speed = if speed >= 1.0 {
        format!("{speed:.0}×")
    } else {
        format!("{speed:.2}×")
    };
    let mut spans = vec![
        Span::styled(
            " SEISMO ",
            style(pal, Ink::Accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            now.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            style(pal, Ink::Text),
        ),
        Span::styled(format!("  {play} {speed}  "), style(pal, Ink::Accent)),
    ];
    if let Some(l) = app.selected() {
        spans.push(Span::styled(
            format!("M{:.1} {}  ", l.event.magnitude, l.place),
            style(pal, Ink::Text),
        ));
        spans.push(Span::styled(
            format!("{} since origin", signed_duration(app.t())),
            style(pal, Ink::Dim),
        ));
    }
    let view = match app.view {
        View::Map => "  [map]".to_owned(),
        View::Slice => format!("  [slice {:.0}°]", app.azimuth_deg),
    };
    spans.push(Span::styled(view, style(pal, Ink::Dim)));
    Line::from(spans)
}

fn footer_line(pal: &Palette) -> Line<'static> {
    let keys = [
        ("?", "keys"),
        ("g", "guide"),
        ("space", "pause"),
        ("←→", "±30s"),
        ("shift ←→", "±5min"),
        ("+-", "speed"),
        ("r", "replay"),
        ("n", "now"),
        ("↑↓ enter", "pick event"),
        ("tab", "map/slice"),
        ("[]", "cut"),
        ("q", "quit"),
    ];
    let mut spans = vec![Span::raw(" ")];
    for (k, what) in keys {
        spans.push(Span::styled(k, style(pal, Ink::Accent)));
        spans.push(Span::styled(format!(" {what}  "), style(pal, Ink::Dim)));
    }
    Line::from(spans)
}

fn draw_side(f: &mut Frame, app: &App, pal: &Palette, area: Rect) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(style(pal, Ink::Dim));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let details = detail_lines(app, pal);
    let detail_h = (details.len() as u16 + 1).min(inner.height / 2);
    let [list_area, detail_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(detail_h)]).areas(inner);

    // The event list, scrolled to keep the cursor in view.
    let shown = app.events.shown();
    let title = Line::from(vec![
        Span::styled(
            format!(
                " EVENTS  {} listed, M{:.1}+ ",
                shown.len(),
                app.events.min_mag()
            ),
            style(pal, Ink::Text).add_modifier(Modifier::BOLD),
        ),
        Span::styled("m/M", style(pal, Ink::Accent)),
    ]);
    let rows = list_area.height.saturating_sub(1) as usize;
    let cursor = shown
        .iter()
        .position(|l| l.key.handle == app.cursor)
        .unwrap_or(0);
    let first = cursor
        .saturating_sub(rows.saturating_sub(1) / 2)
        .min(shown.len().saturating_sub(rows));
    let selected = app.events.selected_handle();
    let now = app.clock.now();
    let width = list_area.width as usize;
    let mut lines = vec![title];
    for l in shown.iter().skip(first).take(rows) {
        let mark = if l.key.handle == selected {
            "●"
        } else if l.made {
            "+"
        } else if l.saved {
            "="
        } else {
            " "
        };
        let text = format!(
            "{mark}M{:<4.1}{:>9} {}",
            l.event.magnitude,
            age(l.event.origin, now),
            l.place
        );
        let text: String = text.chars().take(width).collect();
        let mut st = style(pal, if l.gone { Ink::Dim } else { Ink::Text });
        if l.key.handle == app.cursor {
            st = st.add_modifier(Modifier::REVERSED);
        }
        lines.push(Line::styled(text, st));
    }
    f.render_widget(Paragraph::new(lines), list_area);
    f.render_widget(
        Paragraph::new(details).wrap(Wrap { trim: false }),
        detail_area,
    );
}

fn detail_lines(app: &App, pal: &Palette) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let dim = style(pal, Ink::Dim);
    let text = style(pal, Ink::Text);
    if let Some(l) = app.selected() {
        let e = &l.event;
        out.push(Line::styled(" SELECTED", text.add_modifier(Modifier::BOLD)));
        out.push(Line::styled(format!(" {}", l.place), text));
        out.push(Line::styled(
            format!(" {}", e.origin.format("%Y-%m-%d %H:%M:%S UTC")),
            text,
        ));
        out.push(Line::styled(
            format!(" {:.2}°, {:.2}°  depth {:.0} km", e.lat, e.lon, e.depth_km),
            text,
        ));
        out.push(Line::styled(
            format!(
                " {} {:.1}  {}  {}",
                e.mag_type, e.magnitude, l.usgs_type, l.status
            ),
            dim,
        ));
        if e.kind == sim::EventKind::Earthquake {
            out.push(match e.fault() {
                Some(m) => Line::styled(
                    format!(
                        " fault: {}, strike {:.0}° dip {:.0}° rake {:.0}°",
                        m.style().name(),
                        m.strike_deg,
                        m.dip_deg,
                        m.rake_deg
                    ),
                    text,
                ),
                None => Line::styled(" no fault orientation known: waves leave evenly", dim),
            });
        }
    }
    let stations = app.traces.chosen.len();
    out.push(Line::styled(
        format!(" {stations} stations shown (click the map to change)"),
        dim,
    ));
    if let Some(note) = &app.traces.note {
        out.push(Line::styled(format!(" {note}"), dim));
    }
    out.push(Line::styled(format!(" {}", app.events.feed_line()), dim));
    if let Some(m) = &app.message {
        out.push(Line::styled(format!(" {m}"), dim));
    }
    out.push(Line::styled(" 1D Earth (ak135): no 3D mantle here", dim));
    out
}

fn draw_help(f: &mut Frame, pal: &Palette, area: Rect) {
    let rows: [(&str, &str); 20] = [
        ("space", "play / pause"),
        ("← →", "back / forward 30 s"),
        ("shift ← →  < >", "back / forward 5 min"),
        ("+ -  0", "faster / slower, normal speed"),
        ("r", "replay the selected event from its origin"),
        ("n", "back to now, live"),
        ("↑ ↓  PgUp PgDn", "move in the event list"),
        ("Enter", "select that event (the clock stays put)"),
        ("m M", "lower / raise the minimum magnitude"),
        ("Tab", "world map / slice through the Earth"),
        ("[ ]", "turn the slice's cut 10°"),
        ("click map", "show or hide a station's trace, or place one"),
        ("d", "default stations for the selected event"),
        ("c", "up-down / side-to-side component"),
        ("s", "each trace scaled / one shared scale"),
        ("t", "hide / show the traces"),
        ("l", "CRT / modern look"),
        ("? h F1", "this list of keys"),
        ("g", "the guide: how to use SEISMO, how seismic waves work"),
        ("q Esc", "quit"),
    ];
    let w = 64.min(area.width.saturating_sub(4));
    let h = (rows.len() as u16 + 6).min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    let mut lines: Vec<Line> = rows
        .iter()
        .map(|(k, what)| {
            Line::from(vec![
                Span::styled(format!(" {k:<16}"), style(pal, Ink::Accent)),
                Span::styled(*what, style(pal, Ink::Text)),
            ])
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        " P solid, S dashed; violet glow Rayleigh, dots Love (surface waves).",
        style(pal, Ink::Dim),
    ));
    lines.push(Line::styled(
        " Tab: the guide. Any other key closes this.",
        style(pal, Ink::Dim),
    ));
    let block = Block::default()
        .title(" KEYS ")
        .borders(Borders::ALL)
        .border_style(style(pal, Ink::Accent))
        .style(Style::default().bg(pal.background()));
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

/// The guide over the screen: the list of topics, or one topic to read.
fn draw_guide(f: &mut Frame, app: &mut App, pal: &Palette, area: Rect) {
    let w = 84.min(area.width.saturating_sub(2));
    let h = area.height.saturating_sub(2);
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    let block = Block::default()
        .title(" GUIDE ")
        .borders(Borders::ALL)
        .border_style(style(pal, Ink::Accent))
        .style(Style::default().bg(pal.background()));
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);
    if inner.height < 2 || inner.width < 10 {
        return;
    }
    // The text, and a line of keys at the bottom.
    let rows = usize::from(inner.height - 1);
    let g = &mut app.guide;
    g.rows = rows;
    let (lines, first, hint) = match g.page {
        Page::Topic(i) => {
            let lines = g.topic_lines(i, usize::from(inner.width) - 1, pal);
            g.scroll = g.scroll.min(lines.len().saturating_sub(rows));
            let more = if g.scroll + rows < lines.len() {
                "↓ more  "
            } else {
                ""
            };
            let hint = format!(" {more}↑↓ PgDn scroll  n p next/prev  Esc back");
            (lines, g.scroll, hint)
        }
        _ => {
            let (lines, at) = g.list_lines(pal);
            // The cursor in the middle, unless near either end.
            let first = at
                .saturating_sub(rows / 2)
                .min(lines.len().saturating_sub(rows));
            (
                lines,
                first,
                " ↑↓ choose  Enter read  Tab keys  Esc close".to_owned(),
            )
        }
    };
    let shown: Vec<Line> = lines.into_iter().skip(first).take(rows).collect();
    let [text_area, hint_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    f.render_widget(Paragraph::new(shown), text_area);
    f.render_widget(
        Paragraph::new(Line::styled(hint, style(pal, Ink::Dim))),
        hint_area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn ages_and_durations() {
        let t0 = Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap();
        let ago = |s: i64| age(t0, t0 + chrono::Duration::seconds(s));
        assert_eq!(ago(42), "42s ago");
        assert_eq!(ago(-90), "in 1m");
        assert_eq!(ago(3 * 3600), "3h ago");
        assert_eq!(ago(5 * 86_400), "5d ago");
        assert_eq!(ago(400 * 86_400), "2026-10-15");
        assert_eq!(signed_duration(-45.2), "-45s");
        assert_eq!(signed_duration(312.0), "+5m 12s");
        assert_eq!(signed_duration(3723.0), "+1h 02m 03s");
    }
}
