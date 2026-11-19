//! Small in-memory search. Catalogs are a few thousand items, so a linear
//! scan with a simple score is well under a millisecond and needs no index.

use crate::model::Item;

/// Lowercase, fold common accents, keep letters and digits, single spaces.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = true;
    for c in s.chars().flat_map(char::to_lowercase) {
        let c = fold(c);
        if c.is_alphanumeric() {
            out.push(c);
            space = false;
        } else if c == '\'' || c == '\u{2019}' {
            // "hell's" matches "hells"
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

fn fold(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        '&' => ' ',
        _ => c,
    }
}

/// An item with its searchable text prepared once.
#[derive(Debug, Clone)]
pub struct Indexed {
    pub item: Item,
    title: String,
    words: Vec<String>,
    genres: String,
    description: String,
}

impl Indexed {
    pub fn new(item: Item) -> Indexed {
        let title = normalize(&item.title);
        let words = title.split(' ').map(str::to_string).collect();
        let genres = normalize(&item.genres.join(" "));
        let description = normalize(item.description.as_deref().unwrap_or(""));
        Indexed {
            item,
            title,
            words,
            genres,
            description,
        }
    }
}

pub struct Query {
    pub text: String,
    tokens: Vec<String>,
}

impl Query {
    pub fn new(raw: &str) -> Query {
        let text = normalize(raw);
        let tokens = text
            .split(' ')
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect();
        Query { text, tokens }
    }
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// 0 means no match. Higher is better.
pub fn score(ix: &Indexed, q: &Query) -> u32 {
    if q.is_empty() {
        return 0;
    }
    let t = &ix.title;
    if *t == q.text {
        return 1000;
    }
    if t.starts_with(&q.text) {
        return 800;
    }
    if t.contains(&format!(" {}", q.text)) {
        return 600;
    }
    let all_prefix = q
        .tokens
        .iter()
        .all(|tok| ix.words.iter().any(|w| w.starts_with(tok.as_str())));
    if all_prefix {
        return 400;
    }
    if t.contains(&q.text) {
        return 300;
    }
    let in_title_or_genre =
        |tok: &String| t.contains(tok.as_str()) || ix.genres.contains(tok.as_str());
    if q.tokens.iter().all(in_title_or_genre) {
        return 200;
    }
    // Description matches only count for words of 3+ letters, to avoid noise.
    if q.tokens.iter().all(|tok| {
        in_title_or_genre(tok) || (tok.len() >= 3 && ix.description.contains(tok.as_str()))
    }) {
        return 80;
    }
    0
}

/// Score, filter and sort. Ties go to the shorter title (closer match).
pub fn rank<'a>(items: impl Iterator<Item = &'a Indexed>, q: &Query) -> Vec<(u32, &'a Indexed)> {
    let mut hits: Vec<(u32, &Indexed)> = items
        .map(|ix| (score(ix, q), ix))
        .filter(|(s, _)| *s > 0)
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.title.len().cmp(&b.1.title.len())));
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Kind;

    fn ix(title: &str, genre: &str, desc: &str) -> Indexed {
        let mut it = Item::new("t", title, Kind::Movie, title, "https://example.com".into());
        it.genres = vec![genre.into()];
        it.description = Some(desc.into());
        Indexed::new(it)
    }

    #[test]
    fn normalize_folds() {
        assert_eq!(normalize("  Pokémon: The Movie! "), "pokemon the movie");
        assert_eq!(normalize("Hell's Kitchen"), "hells kitchen");
        assert_eq!(normalize("Law & Order"), "law order");
    }

    #[test]
    fn ordering() {
        let items = [
            ix("Star Trek", "Sci-Fi", ""),
            ix("Star Trek: The Next Generation", "Sci-Fi", ""),
            ix("Lone Star", "Western", ""),
            ix("Cooking Show", "Food", "A star chef cooks"),
            ix("Nothing", "Drama", "unrelated"),
        ];
        let q = Query::new("star trek");
        let r: Vec<&str> = rank(items.iter(), &q)
            .iter()
            .map(|(_, i)| i.item.title.as_str())
            .collect();
        assert_eq!(r, vec!["Star Trek", "Star Trek: The Next Generation"]);

        let q = Query::new("star");
        let r: Vec<&str> = rank(items.iter(), &q)
            .iter()
            .map(|(_, i)| i.item.title.as_str())
            .collect();
        assert_eq!(
            r,
            vec![
                "Star Trek",
                "Star Trek: The Next Generation",
                "Lone Star",
                "Cooking Show"
            ]
        );
    }

    #[test]
    fn prefix_words_and_genre() {
        let items = [
            ix("The Walking Dead", "Horror", ""),
            ix("Pluto News", "News", ""),
        ];
        assert!(score(&items[0], &Query::new("walk dea")) >= 400);
        assert_eq!(score(&items[1], &Query::new("news")), 600);
        assert_eq!(score(&items[0], &Query::new("horror")), 200);
        assert_eq!(score(&items[0], &Query::new("")), 0);
    }
}
