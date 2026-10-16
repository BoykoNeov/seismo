//! `tui`: SEISMO in a terminal. The same simulation, event feed and synthetic seismograms as the
//! desktop app (through `sim`, `feed` and `session`), drawn with text: a braille world map or
//! slice through the Earth, seismograph traces, and the event list. No GPU.
//! Allowed dependencies: `sim`, `feed`, `session`, ratatui. Never `render` or `app`.

mod app;
mod canvas;
mod guide;
mod map;
mod palette;
mod seismograms;
mod slice;
mod ui;

use std::io::Write as _;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::execute;
use sim::seismogram::Component;

use app::{App, Options, View};
use palette::Look;

/// Time between frames when nothing happens.
const FRAME: Duration = Duration::from_millis(66);

const USAGE: &str = "\
seismo-tui: SEISMO in a terminal.

Options:
  --offline             no network: events come from the saved copy only
  --event <id>          select this event: a built-in one (usp000hvnu), one of yours
                        (made-<n>), or a USGS id (looked up, kept and replayed)
  --start-offset <s>    replay the selected event from <s> seconds after its origin
  --speed <x>           clock speed (0.1 to 1000)
  --look crt|modern     the look to start with (default crt)
  --view map|slice      the picture to start with (default map)
  --azimuth <deg>       direction of the slice from the epicentre (default 90)
  --traces <list>       comma-separated: off, side (transverse), shared (one scale),
                        flatcrust (the same flat crust everywhere)
  --dump <cols>x<rows>  draw one frame that size as plain text to standard output, then quit
  --after <s>           with --dump: run this many real seconds first (default 3)
  --ansi                with --dump: in colour (24-bit terminal escape codes)
  --guide <topic>       open the guide at a topic (e.g. map, shadow); `list` for the list
  --help                this text

Keys: press ? in the program; g opens the guide (how to use it, and how seismic waves work).";

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().skip_while(|a| *a != name).nth(1).cloned();
    let flag = |name: &str| args.iter().any(|a| a == name);
    if flag("--help") || flag("-h") {
        println!("{USAGE}");
        return Ok(());
    }
    let mut opts = Options {
        offline: flag("--offline"),
        event: arg("--event"),
        start_offset: arg("--start-offset").and_then(|s| s.parse().ok()),
        speed: arg("--speed").and_then(|s| s.parse().ok()),
        guide: arg("--guide"),
        ..Options::default()
    };
    if arg("--look").as_deref() == Some("modern") {
        opts.look = Look::Modern;
    }
    if arg("--view").as_deref() == Some("slice") {
        opts.view = View::Slice;
    }
    if let Some(az) = arg("--azimuth").and_then(|s| s.parse::<f64>().ok()) {
        opts.azimuth_deg = az;
    }
    for t in arg("--traces").unwrap_or_default().split(',') {
        match t {
            "off" => opts.traces = false,
            "side" => opts.component = Component::Transverse,
            "shared" => opts.shared_scale = true,
            "flatcrust" => opts.flat_crust = true,
            _ => {}
        }
    }
    let feed = feed::spawn(feed::FeedConfig {
        online: !opts.offline,
        ..Default::default()
    });
    let mut app = App::new(&opts, Some(feed))?;

    if let Some(size) = arg("--dump") {
        let (w, h) = size
            .split_once('x')
            .and_then(|(w, h)| Some((w.parse::<u16>().ok()?, h.parse::<u16>().ok()?)))
            .context("--dump wants <cols>x<rows>, e.g. 160x48")?;
        let after = arg("--after")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(3.0);
        let text = dump(&mut app, w, h, after, flag("--ansi"))?;
        std::io::stdout().write_all(text.as_bytes())?;
        return Ok(());
    }
    run(&mut app)
}

/// The interactive loop.
fn run(app: &mut App) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let result = (|| -> anyhow::Result<()> {
        let mut last = Instant::now();
        while !app.quit {
            let now = Instant::now();
            app.tick((now - last).as_secs_f64());
            last = now;
            terminal.draw(|f| ui::draw(f, app))?;
            if event::poll(FRAME)? {
                // Take everything waiting, so a held key does not queue up frames.
                loop {
                    match event::read()? {
                        Event::Key(k) if k.kind == KeyEventKind::Press => app.key(k),
                        Event::Mouse(m) => app.mouse(m),
                        _ => {}
                    }
                    if app.quit || !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
        }
        Ok(())
    })();
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// Run `after` real seconds without a terminal, then draw one `w` × `h` frame as text.
fn dump(app: &mut App, w: u16, h: u16, after: f64, ansi: bool) -> anyhow::Result<String> {
    let start = Instant::now();
    let mut last = start;
    loop {
        let now = Instant::now();
        app.tick((now - last).as_secs_f64());
        last = now;
        if (now - start).as_secs_f64() >= after {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut terminal = Terminal::new(TestBackend::new(w, h))?;
    // Twice: the first frame asks the slice worker for its fans.
    terminal.draw(|f| ui::draw(f, app))?;
    terminal.draw(|f| ui::draw(f, app))?;
    let buf = terminal.backend().buffer();
    Ok(if ansi {
        buffer_ansi(buf)
    } else {
        buffer_text(buf)
    })
}

/// The buffer with 24-bit colour escape codes, one line per row, reset at each line's end.
fn buffer_ansi(buf: &ratatui::buffer::Buffer) -> String {
    use ratatui::style::Color;
    let rgb = |c: Color| match c {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    };
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut last = (None, None);
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            let colours = (rgb(cell.fg), rgb(cell.bg));
            if colours != last {
                if let Some((r, g, b)) = colours.0 {
                    out.push_str(&format!("[38;2;{r};{g};{b}m"));
                }
                if let Some((r, g, b)) = colours.1 {
                    out.push_str(&format!("[48;2;{r};{g};{b}m"));
                }
                last = colours;
            }
            out.push_str(cell.symbol());
        }
        out.push_str(
            "[0m
",
        );
    }
    out
}

fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        for x in 0..buf.area.width {
            line.push_str(buf[(x, y)].symbol());
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn app() -> App {
        let opts = Options {
            start_offset: Some(600.0),
            ..Options::default()
        };
        App::new(&opts, None).unwrap()
    }

    fn frame(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| ui::draw(f, app)).unwrap();
        buffer_text(terminal.backend().buffer())
    }

    fn press(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn draws_at_every_size() {
        let mut app = app();
        for (w, h) in [(20, 5), (60, 16), (80, 24), (120, 40), (240, 70)] {
            let text = frame(&mut app, w, h);
            assert_eq!(text.lines().count(), h as usize);
        }
        let small = frame(&mut app, 20, 5);
        assert!(small.contains("SEISMO needs"));
        let big = frame(&mut app, 160, 48);
        assert!(big.contains("SEISMO") && big.contains("Tōhoku"), "{big}");
        assert!(big.contains("SEISMOGRAMS"));
    }

    #[test]
    fn keys_drive_the_clock_and_views() {
        let mut app = app();
        let t0 = app.t();
        assert!((t0 - 600.0).abs() < 1e-6);
        press(&mut app, KeyCode::Char(' '));
        assert!(!app.clock.playing());
        press(&mut app, KeyCode::Right);
        assert!((app.t() - 630.0).abs() < 1e-6);
        press(&mut app, KeyCode::Char('<'));
        assert!((app.t() - 330.0).abs() < 1e-6);
        press(&mut app, KeyCode::Char('r'));
        assert!(app.t().abs() < 1e-6 && app.clock.playing());
        press(&mut app, KeyCode::Char('+'));
        assert_eq!(app.clock.speed(), 2.0);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.view, View::Slice);
        press(&mut app, KeyCode::Char(']'));
        assert_eq!(app.azimuth_deg, 100.0);
        press(&mut app, KeyCode::Char('?'));
        assert!(app.help);
        press(&mut app, KeyCode::Char('x'));
        assert!(!app.help && !app.quit);
        // Tab in the key list goes to the guide; its keys stay in the overlay until Esc.
        press(&mut app, KeyCode::Char('?'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.guide.page, guide::Page::Topic(1));
        // Space pages the topic; the clock is not touched.
        let playing = app.clock.playing();
        press(&mut app, KeyCode::Char(' '));
        assert!(app.help && app.clock.playing() == playing);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Esc);
        assert!(!app.help && !app.quit);
        press(&mut app, KeyCode::Char('g'));
        assert!(app.help && app.guide.page == guide::Page::Topics);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
    }

    #[test]
    fn slice_view_draws_once_the_fans_are_in() {
        let mut app = app();
        app.view = View::Slice;
        let start = Instant::now();
        loop {
            let text = frame(&mut app, 160, 48);
            if !text.contains("tracing rays") {
                assert!(text.contains("epicentre") && text.contains("CMB"), "{text}");
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(120),
                "fans never arrived"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn traces_appear_once_the_ray_tubes_are_in() {
        let mut app = app();
        let start = Instant::now();
        while app.traces.pending || app.traces.shown().is_empty() {
            assert!(
                start.elapsed() < Duration::from_secs(120),
                "traces never arrived"
            );
            std::thread::sleep(Duration::from_millis(100));
            app.tick(0.0);
        }
        let text = frame(&mut app, 160, 48);
        // Default stations are labelled by code; the P arrival is marked on a trace.
        let code = &app.traces.stations[app.traces.chosen[0]].code;
        assert!(text.contains(code.as_str()), "{text}");
    }
}
