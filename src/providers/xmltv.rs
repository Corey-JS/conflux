//! Reader for XMLTV, the guide format most TV tools share. Conflux reads only
//! programmes that have an image: their channel's names, times, title,
//! episode name, description, rating and image.

use std::collections::HashMap;
use std::io::BufRead;

use anyhow::Result;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::model::{clean, is_web_url, iso_to_unix};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Programme {
    pub start: u64,
    pub stop: u64,
    pub title: String,
    pub subtitle: Option<String>,
    pub description: Option<String>,
    pub rating: Option<String>,
    pub image: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Channel {
    pub names: Vec<String>,
    /// In start order.
    pub programmes: Vec<Programme>,
}

/// Which text element is being read.
#[derive(Clone, Copy, PartialEq)]
enum Field {
    None,
    Name,
    Title,
    Subtitle,
    Description,
    Rating,
}

/// Channels by XMLTV id, keeping only programmes with an image that overlap
/// `from..to` (Unix seconds), so a two-week guide doesn't all sit in memory.
pub fn parse(input: impl BufRead, from: u64, to: u64) -> Result<HashMap<String, Channel>> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(true);
    let mut out: HashMap<String, Channel> = HashMap::new();
    let mut buf = Vec::new();
    // The channel or programme being read, and its channel id.
    let mut channel: Option<String> = None;
    let mut prog: Option<(String, Programme)> = None;
    let mut field = Field::None;
    let mut text = String::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                field = Field::None;
                match e.local_name().as_ref() {
                    b"channel" => channel = attr(&e, "id"),
                    b"programme" => prog = start_programme(&e),
                    b"display-name" if channel.is_some() => field = Field::Name,
                    b"title" if prog.is_some() => field = Field::Title,
                    b"sub-title" if prog.is_some() => field = Field::Subtitle,
                    b"desc" if prog.is_some() => field = Field::Description,
                    // `<rating><value>TV-14</value></rating>`, or the rating
                    // as the element's own text (Roku's guide).
                    b"rating" | b"value" if prog.is_some() => field = Field::Rating,
                    b"icon" => icon(&e, prog.as_mut()),
                    _ => {}
                }
                text.clear();
            }
            Event::Empty(e) if e.local_name().as_ref() == b"icon" => icon(&e, prog.as_mut()),
            Event::Text(t) if field != Field::None => text.push_str(&t.unescape()?),
            Event::CData(t) if field != Field::None => {
                text.push_str(&String::from_utf8_lossy(&t.into_inner()));
            }
            Event::End(e) => {
                let v = match field {
                    Field::None => None,
                    _ => clean(Some(std::mem::take(&mut text))),
                };
                match (e.local_name().as_ref(), field, &mut prog) {
                    (b"display-name", Field::Name, _) => {
                        if let (Some(id), Some(name)) = (&channel, v) {
                            out.entry(id.clone()).or_default().names.push(name);
                        }
                    }
                    (b"title", Field::Title, Some((_, p))) if p.title.is_empty() => {
                        p.title = v.unwrap_or_default();
                    }
                    (b"sub-title", Field::Subtitle, Some((_, p))) => p.subtitle = p.subtitle.take().or(v),
                    (b"desc", Field::Description, Some((_, p))) => {
                        p.description = p.description.take().or(v);
                    }
                    (b"rating" | b"value", Field::Rating, Some((_, p))) => {
                        p.rating = p.rating.take().or(v.map(|r| tv_rating(&r)));
                    }
                    (b"channel", ..) => channel = None,
                    (b"programme", ..) => {
                        if let Some((id, p)) = prog.take() {
                            let keep = !p.title.is_empty()
                                && !p.image.is_empty()
                                && p.stop > p.start
                                && p.stop > from
                                && p.start < to;
                            if keep {
                                out.entry(id).or_default().programmes.push(p);
                            }
                        }
                    }
                    _ => {}
                }
                field = Field::None;
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    for c in out.values_mut() {
        c.programmes.sort_by_key(|p| p.start);
    }
    Ok(out)
}

/// US TV ratings with their dash ("TV14" becomes "TV-14"); others as given.
fn tv_rating(r: &str) -> String {
    match r.strip_prefix("TV") {
        Some(rest) if (1..=3).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_alphanumeric()) => {
            format!("TV-{rest}")
        }
        _ => r.to_string(),
    }
}

fn attr(e: &BytesStart, name: &str) -> Option<String> {
    let a = e.try_get_attribute(name).ok()??;
    Some(a.unescape_value().ok()?.into_owned())
}

fn start_programme(e: &BytesStart) -> Option<(String, Programme)> {
    let p = Programme {
        start: time(&attr(e, "start")?)?,
        stop: time(&attr(e, "stop")?)?,
        ..Default::default()
    };
    Some((attr(e, "channel")?, p))
}

/// A programme's first web image; channel icons are skipped.
fn icon(e: &BytesStart, prog: Option<&mut (String, Programme)>) {
    if let Some((_, p)) = prog
        && p.image.is_empty()
        && let Some(src) = attr(e, "src").filter(|s| is_web_url(s))
    {
        p.image = src;
    }
}

/// "20261120082100 +0000" (the offset is optional) to Unix seconds.
pub fn time(s: &str) -> Option<u64> {
    let (digits, offset) = match s.trim().split_once(' ') {
        Some((d, o)) => (d, Some(o.trim())),
        None => (s.trim(), None),
    };
    let d = digits.get(..14).filter(|d| d.bytes().all(|b| b.is_ascii_digit()))?;
    let iso = format!(
        "{}-{}-{}T{}:{}:{}",
        &d[..4],
        &d[4..6],
        &d[6..8],
        &d[8..10],
        &d[10..12],
        &d[12..14]
    );
    let local = iso_to_unix(&iso)? as i64;
    let shift = match offset {
        None => 0,
        Some(o) => {
            let (sign, hm) = match o.as_bytes().first()? {
                b'+' => (1, &o[1..]),
                b'-' => (-1, &o[1..]),
                _ => return None,
            };
            if hm.len() != 4 || !hm.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let mins: i64 = hm[..2].parse::<i64>().ok()? * 60 + hm[2..].parse::<i64>().ok()?;
            sign * mins * 60
        }
    };
    u64::try_from(local - shift).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_with_offsets() {
        assert_eq!(time("20261120082100 +0000"), Some(1_795_162_860));
        assert_eq!(time("20261120032100 -0500"), Some(1_795_162_860));
        assert_eq!(time("20261120082100"), Some(1_795_162_860));
        assert_eq!(time("2026112008"), None);
        assert_eq!(time("20261120082100 0500"), None);
    }

    #[test]
    fn ratings_either_way_with_a_dash() {
        let xml = r#"<tv>
<programme start="20261120080000" stop="20261120090000" channel="a"><title>A</title>
  <rating system="USA Parental Rating">TV14</rating><icon src="https://x/a.jpg"/></programme>
<programme start="20261120090000" stop="20261120100000" channel="a"><title>B</title>
  <rating system="US"><value>TV-PG</value></rating><icon src="https://x/b.jpg"/></programme>
<programme start="20261120100000" stop="20261120110000" channel="a"><title>C</title>
  <rating system="MPAA">PG-13</rating><icon src="https://x/c.jpg"/></programme>
</tv>"#;
        let g = parse(xml.as_bytes(), 0, u64::MAX).unwrap();
        let ratings: Vec<_> = g["a"].programmes.iter().map(|p| p.rating.as_deref()).collect();
        assert_eq!(ratings, [Some("TV-14"), Some("TV-PG"), Some("PG-13")]);
        assert_eq!(tv_rating("TVMA"), "TV-MA");
        assert_eq!(tv_rating("TVY7"), "TV-Y7");
        assert_eq!(tv_rating("TV Land"), "TV Land");
    }

    #[test]
    fn keeps_programmes_with_images_in_the_window() {
        let xml = "\u{feff}<?xml version=\"1.0\"?>
<tv>
  <channel id=\"a&amp;b\"><display-name>A&amp;E Live</display-name>
    <icon src=\"https://x/logo.png\"/></channel>
  <programme start=\"20261120080000 +0000\" stop=\"20261120090000 +0000\" channel=\"a&amp;b\">
    <title>Show</title><sub-title>Pilot</sub-title><desc><![CDATA[First.]]></desc>
    <rating system=\"US\"><value>TV-14</value></rating>
    <icon src=\"javascript:alert(1)\"/><icon src=\"https://x/show.jpg\"/>
  </programme>
  <programme start=\"20261120090000 +0000\" stop=\"20261120100000 +0000\" channel=\"a&amp;b\">
    <title>No art</title>
  </programme>
  <programme start=\"20261201090000 +0000\" stop=\"20261201100000 +0000\" channel=\"a&amp;b\">
    <title>Too late</title><icon src=\"https://x/late.jpg\"/>
  </programme>
</tv>";
        let from = time("20261120000000").unwrap();
        let to = time("20261121000000").unwrap();
        let g = parse(xml.as_bytes(), from, to).unwrap();
        let c = &g["a&b"];
        assert_eq!(c.names, ["A&E Live"]);
        assert_eq!(
            c.programmes,
            [Programme {
                start: 1_795_161_600,
                stop: 1_795_165_200,
                title: "Show".into(),
                subtitle: Some("Pilot".into()),
                description: Some("First.".into()),
                rating: Some("TV-14".into()),
                image: "https://x/show.jpg".into(),
            }]
        );
    }
}
