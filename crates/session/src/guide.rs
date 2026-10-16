//! The guide: how to use SEISMO, what each picture shows, and short lessons on earthquakes and
//! seismic waves. Shared by both front ends, which only draw it.
//!
//! The text is `guide.txt` beside this file, embedded at build time, in a small markup:
//!
//! - `=== id | Title | part` starts a topic; part is `start`, `screen` or `learn`. A fourth field
//!   `desktop` or `terminal` keeps the topic to that front end.
//! - Blocks are separated by blank lines: `## heading`, `- bullet` (indented lines continue it),
//!   or a paragraph. A block whose first line starts with `@desktop ` or `@terminal ` shows only
//!   in that front end. Lines starting `//` are comments.
//! - Inline: `**strong**`, `[[topic-id]]` (a link titled after the topic) or
//!   `[[topic-id|text]]`, and `{name}` placeholders.
//!
//! Every number about the Earth (depths, speeds, travel times, shadow edges) is a placeholder
//! filled from [`sim::facts::ModelFacts`], and every app setting (the follow magnitude, the
//! trace cap, …) from its constant, so the text cannot drift from what the app simulates. A test
//! rejects a hand-written distance, depth, speed or travel time.

use std::collections::BTreeMap;

use sim::EventKind;
use sim::facts::{FAR_DEG, ModelFacts, NEAR_DEG};

use crate::clock::{MAX_SPEED, MIN_SPEED};
use crate::events::{DEFAULT_MIN_MAG, FOLLOW_MIN_MAG};
use crate::traces::MAX_TRACES;

/// The guide's text, before placeholders are filled.
pub const SOURCE: &str = include_str!("guide.txt");

/// Which program shows the guide: the two have different controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Front {
    Desktop,
    Terminal,
}

impl Front {
    fn tag(self) -> &'static str {
        match self {
            Front::Desktop => "desktop",
            Front::Terminal => "terminal",
        }
    }
}

/// The guide's three parts, in reading order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Part {
    Start,
    Screen,
    Learn,
}

impl Part {
    pub const ALL: [Part; 3] = [Part::Start, Part::Screen, Part::Learn];

    pub fn title(self) -> &'static str {
        match self {
            Part::Start => "Getting started",
            Part::Screen => "What you see",
            Part::Learn => "Learn: earthquakes and waves",
        }
    }

    fn parse(s: &str) -> Option<Part> {
        match s {
            "start" => Some(Part::Start),
            "screen" => Some(Part::Screen),
            "learn" => Some(Part::Learn),
            _ => None,
        }
    }
}

/// A run of text in a paragraph or bullet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Span {
    Text(String),
    Strong(String),
    /// A link to another topic of the guide.
    Link {
        id: String,
        text: String,
    },
}

impl Span {
    pub fn text(&self) -> &str {
        match self {
            Span::Text(t) | Span::Strong(t) | Span::Link { text: t, .. } => t,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading(String),
    Para(Vec<Span>),
    Bullet(Vec<Span>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Topic {
    pub id: String,
    pub title: String,
    pub part: Part,
    pub blocks: Vec<Block>,
}

impl Topic {
    /// The opening paragraph: the topic in a few sentences, for the inline explanations.
    pub fn lead(&self) -> &[Span] {
        self.blocks
            .iter()
            .find_map(|b| match b {
                Block::Para(spans) => Some(spans.as_slice()),
                _ => None,
            })
            .unwrap_or(&[])
    }

    /// All of the topic's text, for searching.
    pub fn plain(&self) -> String {
        let mut out = self.title.clone();
        for b in &self.blocks {
            out.push('\n');
            match b {
                Block::Heading(h) => out.push_str(h),
                Block::Para(spans) | Block::Bullet(spans) => {
                    spans.iter().for_each(|s| out.push_str(s.text()));
                }
            }
        }
        out
    }
}

/// The values of the `{name}` placeholders.
#[derive(Debug, Clone, Default)]
pub struct Values(BTreeMap<String, String>);

impl Values {
    /// The model's numbers and the session's settings. A front end adds its own with
    /// [`Values::set`].
    pub fn new(facts: &ModelFacts) -> Self {
        let mut v = Values::default();
        let km = |d: f64| format!("{d:.0}");
        let speed = |s: f64| format!("{s:.1}");
        let minutes = |s: f64| format!("{:.1}", s / 60.0);
        // Degrees and magnitudes: whole numbers without a decimal point.
        let num = |d: f64| {
            let s = format!("{d:.1}");
            s.strip_suffix(".0").map_or(s.clone(), str::to_owned)
        };
        let f = facts;
        v.set("model", &f.model);
        v.set("near_deg", num(NEAR_DEG));
        v.set("far_deg", num(FAR_DEG));
        v.set("earth_radius_km", km(f.earth_radius_km));
        v.set(
            "km_per_deg",
            km(std::f64::consts::TAU * f.earth_radius_km / 360.0),
        );
        for (name, d) in [
            ("moho_km", f.moho_km),
            ("d410_km", f.d410_km),
            ("d660_km", f.d660_km),
            ("cmb_km", f.cmb_km),
            ("icb_km", f.icb_km),
        ] {
            if let Some(d) = d {
                v.set(name, km(d));
            }
        }
        if let Some(d) = f.cmb_km {
            v.set("core_radius_km", km(f.earth_radius_km - d));
        }
        if let Some(d) = f.icb_km {
            v.set("inner_core_radius_km", km(f.earth_radius_km - d));
        }
        v.set("vp_surface", speed(f.surface.vp_km_s));
        v.set("vs_surface", speed(f.surface.vs_km_s));
        for (name, s) in [
            ("upper_mantle", f.upper_mantle),
            ("lowest_mantle", f.lowest_mantle),
            ("outer_core", f.outer_core),
            ("inner_core", f.inner_core),
        ] {
            if let Some(s) = s {
                v.set(&format!("vp_{name}"), speed(s.vp_km_s));
                v.set(&format!("vs_{name}"), speed(s.vs_km_s));
            }
        }
        for (name, t) in [
            ("p30_min", f.p30_s),
            ("p90_min", f.p90_s),
            ("s30_min", f.s30_s),
            ("s90_min", f.s90_s),
            ("pkikp180_min", f.pkikp180_s),
        ] {
            if let Some(t) = t {
                v.set(name, minutes(t));
            }
        }
        if let (Some(p), Some(s)) = (f.p30_s, f.s30_s) {
            v.set("sp30_min", minutes(s - p));
        }
        if let (Some(p), Some(s)) = (f.p90_s, f.s90_s) {
            v.set("sp90_min", minutes(s - p));
        }
        for (name, d) in [
            ("p_last_deg", f.p_last_deg),
            ("pkp_first_deg", f.pkp_first_deg),
            ("s_last_deg", f.s_last_deg),
        ] {
            if let Some(d) = d {
                v.set(name, num(d));
            }
        }
        v.set("rayleigh_slow", speed(f.rayleigh_group_km_s.0));
        v.set("rayleigh_fast", speed(f.rayleigh_group_km_s.1));
        v.set("love_slow", speed(f.love_group_km_s.0));
        v.set("love_fast", speed(f.love_group_km_s.1));
        v.set("surface_period_min_s", num(f.surface_band_s.0));
        v.set("surface_period_max_s", num(f.surface_band_s.1));
        if let Some(c) = &f.crust {
            v.set("crust_model", &c.model);
            v.set("crust_thin_km", km(c.thinnest_km));
            v.set("crust_thick_km", km(c.thickest_km));
            // Early or late by: the sign is in the words around it.
            v.set(
                "crust_thin_p_s",
                num((c.thinnest_p_s.abs() * 10.0).round() / 10.0),
            );
            v.set(
                "crust_thick_p_s",
                num((c.thickest_p_s.abs() * 10.0).round() / 10.0),
            );
        }
        if let Some(e) = &f.ellipticity {
            v.set("ellip_p_s", num((e.largest_p_s * 10.0).round() / 10.0));
            v.set("ellip_largest_s", num((e.largest_s * 10.0).round() / 10.0));
        }
        v.set(
            "deepest_quake_km",
            km(EventKind::Earthquake.profile().max_depth_km),
        );
        v.set("follow_mag", num(FOLLOW_MIN_MAG));
        v.set("listen_speed", num(crate::sound::DEFAULT_SPEED));
        v.set("default_min_mag", num(DEFAULT_MIN_MAG));
        v.set("surface_map_mag", num(sim::SURFACE_MAP_MIN_MAG));
        v.set("max_traces", MAX_TRACES.to_string());
        v.set("min_speed", num(MIN_SPEED));
        v.set("max_speed", num(MAX_SPEED));
        v
    }

    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        self.0.insert(name.to_owned(), value.into());
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }
}

/// The guide as one front end shows it.
#[derive(Debug, Clone)]
pub struct Guide {
    topics: Vec<Topic>,
}

impl Guide {
    /// The embedded guide for `front`, with its placeholders filled.
    pub fn new(front: Front, values: &Values) -> Result<Self, String> {
        Self::parse(SOURCE, front, values)
    }

    /// Parse `source` and keep what `front` shows. Fails on a malformed topic line, an unknown
    /// placeholder or a link to a topic that does not exist. Links to topics the other front end
    /// has become plain text.
    pub fn parse(source: &str, front: Front, values: &Values) -> Result<Self, String> {
        let all = parse_topics(source)?;
        let ids: Vec<&str> = all.iter().map(|t| t.0.id.as_str()).collect();
        let mut topics = Vec::new();
        for (head, raw_blocks) in &all {
            if head.only.is_some_and(|f| f != front) {
                continue;
            }
            let mut blocks = Vec::new();
            for raw in raw_blocks {
                if raw.only.is_some_and(|f| f != front) {
                    continue;
                }
                let text = fill(&raw.text, values).map_err(|e| format!("{}: {e}", head.id))?;
                blocks.push(match raw.kind {
                    Kind::Heading => Block::Heading(text),
                    Kind::Para => {
                        Block::Para(spans(&text).map_err(|e| format!("{}: {e}", head.id))?)
                    }
                    Kind::Bullet => {
                        Block::Bullet(spans(&text).map_err(|e| format!("{}: {e}", head.id))?)
                    }
                });
            }
            topics.push(Topic {
                id: head.id.clone(),
                title: head.title.clone(),
                part: head.part,
                blocks,
            });
        }
        // Links: to a topic shown here, titled after it unless the text says otherwise; to one
        // the other front end has, plain text; to none, an error.
        let titles: BTreeMap<String, String> = topics
            .iter()
            .map(|t| (t.id.clone(), t.title.clone()))
            .collect();
        for t in &mut topics {
            for b in &mut t.blocks {
                let (Block::Para(spans) | Block::Bullet(spans)) = b else {
                    continue;
                };
                for s in spans.iter_mut() {
                    let Span::Link { id, text } = s else {
                        continue;
                    };
                    if !ids.contains(&id.as_str()) {
                        return Err(format!("{}: link to unknown topic {id}", t.id));
                    }
                    match titles.get(id.as_str()) {
                        Some(title) if text.is_empty() => *text = title.clone(),
                        Some(_) => {}
                        None => {
                            let text = if text.is_empty() {
                                id.clone()
                            } else {
                                text.clone()
                            };
                            *s = Span::Text(text);
                        }
                    }
                }
            }
        }
        topics.sort_by_key(|t| t.part);
        Ok(Self { topics })
    }

    /// Topics in reading order: by part, then as written.
    pub fn topics(&self) -> &[Topic] {
        &self.topics
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.topics.iter().position(|t| t.id == id)
    }

    pub fn topic(&self, id: &str) -> Option<&Topic> {
        self.index_of(id).map(|i| &self.topics[i])
    }

    /// Indices of the topics matching every word of `query` (ignoring case): those with a match
    /// in the title first, then the rest, each in reading order. An empty query matches all.
    pub fn search(&self, query: &str) -> Vec<usize> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let has_all = |text: &str| {
            let text = text.to_lowercase();
            words.iter().all(|w| text.contains(w.as_str()))
        };
        let (mut titled, mut rest) = (Vec::new(), Vec::new());
        for (i, t) in self.topics.iter().enumerate() {
            if has_all(&t.title) {
                titled.push(i);
            } else if has_all(&t.plain()) {
                rest.push(i);
            }
        }
        titled.extend(rest);
        titled
    }
}

struct Head {
    id: String,
    title: String,
    part: Part,
    only: Option<Front>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Heading,
    Para,
    Bullet,
}

struct RawBlock {
    kind: Kind,
    text: String,
    only: Option<Front>,
}

fn front_tag(s: &str) -> Option<Front> {
    [Front::Desktop, Front::Terminal]
        .into_iter()
        .find(|f| f.tag() == s)
}

/// Topics and their blocks, before placeholders and inline markup.
fn parse_topics(source: &str) -> Result<Vec<(Head, Vec<RawBlock>)>, String> {
    let mut topics: Vec<(Head, Vec<RawBlock>)> = Vec::new();
    let mut current: Option<RawBlock> = None;
    let flush = |topics: &mut Vec<(Head, Vec<RawBlock>)>, b: &mut Option<RawBlock>| {
        if let (Some(b), Some(t)) = (b.take(), topics.last_mut()) {
            t.1.push(b);
        }
    };
    for (n, line) in source.lines().enumerate() {
        let n = n + 1;
        let line = line.trim_end();
        if line.starts_with("//") {
            continue;
        }
        if let Some(head) = line.strip_prefix("===") {
            flush(&mut topics, &mut current);
            let fields: Vec<&str> = head.split('|').map(str::trim).collect();
            let (id, title, part) = match fields[..] {
                [id, title, part] | [id, title, part, _] => (id, title, part),
                _ => return Err(format!("line {n}: expected `=== id | Title | part`")),
            };
            let part = Part::parse(part).ok_or(format!("line {n}: unknown part {part}"))?;
            let only = match fields.get(3) {
                Some(f) => Some(front_tag(f).ok_or(format!("line {n}: unknown front {f}"))?),
                None => None,
            };
            if id.is_empty() || topics.iter().any(|t| t.0.id == id) {
                return Err(format!("line {n}: empty or repeated topic id {id}"));
            }
            topics.push((
                Head {
                    id: id.to_owned(),
                    title: title.to_owned(),
                    part,
                    only,
                },
                Vec::new(),
            ));
            continue;
        }
        if line.is_empty() {
            flush(&mut topics, &mut current);
            continue;
        }
        if topics.is_empty() {
            return Err(format!("line {n}: text before the first topic"));
        }
        let continues = current.as_ref().is_some_and(|b| match b.kind {
            Kind::Heading => false,
            Kind::Para => !line.starts_with(['-', '#', '@']),
            Kind::Bullet => line.starts_with(' '),
        });
        if continues {
            let b = current.as_mut().expect("checked");
            b.text.push(' ');
            b.text.push_str(line.trim());
            continue;
        }
        flush(&mut topics, &mut current);
        let (only, rest) = match line.strip_prefix('@') {
            Some(tagged) => {
                let (tag, rest) = tagged.split_once(' ').unwrap_or((tagged, ""));
                let f = front_tag(tag).ok_or(format!("line {n}: unknown front @{tag}"))?;
                (Some(f), rest.trim_start())
            }
            None => (None, line),
        };
        let (kind, text) = if let Some(h) = rest.strip_prefix("## ") {
            (Kind::Heading, h)
        } else if let Some(b) = rest.strip_prefix("- ") {
            (Kind::Bullet, b)
        } else {
            (Kind::Para, rest)
        };
        current = Some(RawBlock {
            kind,
            text: text.trim().to_owned(),
            only,
        });
    }
    flush(&mut topics, &mut current);
    Ok(topics)
}

/// `text` with every `{name}` replaced by its value.
fn fill(text: &str, values: &Values) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after.find('}').ok_or("unclosed {")?;
        let name = &after[..close];
        out.push_str(
            values
                .get(name)
                .ok_or_else(|| format!("unknown placeholder {{{name}}}"))?,
        );
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Inline markup: `**strong**` and `[[id]]` / `[[id|text]]` links.
fn spans(text: &str) -> Result<Vec<Span>, String> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    loop {
        let strong = rest.find("**");
        let link = rest.find("[[");
        let next = match (strong, link) {
            (Some(s), Some(l)) => Some(s.min(l)),
            (s, l) => s.or(l),
        };
        let Some(at) = next else {
            plain.push_str(rest);
            break;
        };
        plain.push_str(&rest[..at]);
        if !plain.is_empty() {
            out.push(Span::Text(std::mem::take(&mut plain)));
        }
        if Some(at) == strong {
            let inner = &rest[at + 2..];
            let end = inner.find("**").ok_or("unclosed **")?;
            out.push(Span::Strong(inner[..end].to_owned()));
            rest = &inner[end + 2..];
        } else {
            let inner = &rest[at + 2..];
            let end = inner.find("]]").ok_or("unclosed [[")?;
            let (id, text) = inner[..end].split_once('|').unwrap_or((&inner[..end], ""));
            out.push(Span::Link {
                id: id.trim().to_owned(),
                text: text.trim().to_owned(),
            });
            rest = &inner[end + 2..];
        }
    }
    if !plain.is_empty() {
        out.push(Span::Text(plain));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values() -> Values {
        let (table, model, dispersion) = crate::assets::earth().unwrap();
        let mut facts = ModelFacts::new(&table, &model, &dispersion);
        facts.add_crust(&crate::assets::crust(&model).unwrap());
        facts.add_ellipticity(&crate::assets::ellipticity().unwrap());
        let mut v = Values::new(&facts);
        // What the front ends add.
        v.set("map_cap", "32");
        v.set("stations", "150");
        v
    }

    #[test]
    fn the_embedded_guide_parses_for_both_front_ends() {
        let v = values();
        for front in [Front::Desktop, Front::Terminal] {
            let g = Guide::new(front, &v).unwrap_or_else(|e| panic!("{front:?}: {e}"));
            assert!(g.topics().len() > 10);
            for t in g.topics() {
                assert!(!t.lead().is_empty(), "{} has no opening paragraph", t.id);
                // Every link leads somewhere this front end shows.
                for b in &t.blocks {
                    if let Block::Para(s) | Block::Bullet(s) = b {
                        for s in s {
                            if let Span::Link { id, .. } = s {
                                assert!(g.topic(id).is_some(), "{} links to {id}", t.id);
                            }
                        }
                    }
                }
                let text = t.plain();
                assert!(
                    !text.contains(['{', '}']) && !text.contains("**"),
                    "{}",
                    t.id
                );
            }
            // Parts in reading order.
            assert!(g.topics().windows(2).all(|w| w[0].part <= w[1].part));
        }
    }

    #[test]
    fn the_front_ends_get_their_own_topics() {
        let v = values();
        let desktop = Guide::new(Front::Desktop, &v).unwrap();
        let terminal = Guide::new(Front::Terminal, &v).unwrap();
        assert!(desktop.topic("make").is_some() && terminal.topic("make").is_none());
        assert!(desktop.topic("map").is_some() && terminal.topic("map").is_some());
    }

    /// Topics the desktop app opens from its `?` glyphs.
    #[test]
    fn topics_the_app_opens_exist() {
        let g = Guide::new(Front::Desktop, &values()).unwrap();
        for id in [
            "welcome",
            "first-steps",
            "clock",
            "events",
            "search",
            "make",
            "map",
            "slice",
            "seismograms",
            "event-panel",
            "mantle-control",
            "looks",
            "magnitude",
            "fault",
        ] {
            assert!(g.topic(id).is_some(), "{id}");
        }
    }

    /// The project keeps velocities, travel times and layer depths in `sim` and `tables`: the
    /// guide quotes them through placeholders, never as typed numbers.
    #[test]
    fn no_hand_written_depths_speeds_or_travel_times() {
        let units = ["km", "°", "min", "minute", "kilomet", "second"];
        for (n, line) in SOURCE.lines().enumerate() {
            // Placeholders are fine; look at what is left.
            let mut text = String::new();
            let mut depth = 0;
            for c in line.chars() {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ if depth == 0 => text.push(c),
                    _ => text.push('#'),
                }
            }
            let chars: Vec<char> = text.chars().collect();
            for i in 0..chars.len() {
                if !chars[i].is_ascii_digit() {
                    continue;
                }
                let after: String = chars[i + 1..].iter().collect();
                let after = after.trim_start_matches([' ', '.', ',']);
                assert!(
                    !units.iter().any(|u| after.starts_with(u)),
                    "guide.txt line {}: write this number as a placeholder: {line}",
                    n + 1
                );
            }
        }
    }

    #[test]
    fn markup() {
        let src = "=== a | Alpha | start\nLead with **bold** and [[b]].\n\n- one\n  two\n\
                   @terminal Only here.\n\n## Head\n=== b | Beta | learn | desktop\nB {x}.\n";
        let mut v = Values::default();
        v.set("x", "42");
        let d = Guide::parse(src, Front::Desktop, &v).unwrap();
        let a = d.topic("a").unwrap();
        assert_eq!(
            a.lead(),
            [
                Span::Text("Lead with ".into()),
                Span::Strong("bold".into()),
                Span::Text(" and ".into()),
                Span::Link {
                    id: "b".into(),
                    text: "Beta".into()
                },
                Span::Text(".".into()),
            ]
        );
        assert_eq!(
            a.blocks[1],
            Block::Bullet(vec![Span::Text("one two".into())])
        );
        assert_eq!(a.blocks[2], Block::Heading("Head".into()));
        assert_eq!(d.topic("b").unwrap().plain(), "Beta\nB 42.");
        // The terminal has no topic b: the link becomes text, and the tagged block shows.
        let t = Guide::parse(src, Front::Terminal, &v).unwrap();
        let a = t.topic("a").unwrap();
        assert!(t.topic("b").is_none());
        assert_eq!(a.lead()[3], Span::Text("b".into()));
        assert_eq!(
            a.blocks[2],
            Block::Para(vec![Span::Text("Only here.".into())])
        );
        // Errors.
        assert!(Guide::parse("=== a | A | start\n{nope}\n", Front::Desktop, &v).is_err());
        assert!(Guide::parse("=== a | A | start\n[[zz]]\n", Front::Desktop, &v).is_err());
        assert!(Guide::parse("=== a | A | middle\nx\n", Front::Desktop, &v).is_err());
    }

    #[test]
    fn search_puts_title_matches_first() {
        let src = "=== a | Waves | start\nAbout the core.\n\n=== b | Core | start\nDeep.\n";
        let g = Guide::parse(src, Front::Desktop, &Values::default()).unwrap();
        assert_eq!(g.search("core"), vec![1, 0]);
        assert_eq!(g.search("WAVES core"), vec![0]);
        assert_eq!(g.search(""), vec![0, 1]);
        assert!(g.search("mantle").is_empty());
    }
}
