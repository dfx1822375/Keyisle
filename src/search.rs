//! Bounded local search over public credential metadata only.
use pinyin::ToPinyinMulti;

fn normalized(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(c as u32 - 0xfee0).unwrap(),
            '\u{3000}' => ' ',
            _ => c,
        })
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ü' { 'v' } else { c })
        .collect()
}
fn compact(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).collect()
}

#[derive(Clone, Default)]
struct Spelling {
    full: String,
    initials: String,
    full_positions: Vec<usize>,
    initial_positions: Vec<usize>,
}
impl Spelling {
    fn push(&mut self, syllable: &str, position: usize) {
        self.full.push_str(syllable);
        self.initials.push(syllable.chars().next().unwrap());
        self.full_positions
            .extend(std::iter::repeat_n(position, syllable.chars().count()));
        self.initial_positions.push(position);
    }
}
fn pronunciations(text: &str) -> Vec<Spelling> {
    let mut variants = vec![Spelling::default()];
    for (position, ch) in text.chars().enumerate() {
        let syllables: Vec<_> = ch
            .to_pinyin_multi()
            .map(|p| p.into_iter().map(|v| normalized(v.plain())).collect())
            .unwrap_or_else(|| vec![ch.to_string()]);
        if syllables.len() == 1 || variants.len() == 16 {
            for variant in &mut variants {
                variant.push(&syllables[0], position);
            }
            continue;
        }
        let mut next = Vec::new();
        // shortcut: bound polyphonic combinations to 16; add a phrase dictionary if real searches need more.
        for syllable in &syllables {
            for variant in &variants {
                if next.len() == 16 {
                    break;
                }
                let mut variant = variant.clone();
                variant.push(syllable, position);
                next.push(variant);
            }
            if next.len() == 16 {
                break;
            }
        }
        variants = next;
    }
    variants
}

fn one_edit(a: &[u8], b: &[u8]) -> bool {
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let mut i = 0;
    while i < a.len().min(b.len()) && a[i] == b[i] {
        i += 1;
    }
    if i == a.len().min(b.len()) {
        return true;
    }
    if a.len() == b.len() {
        a[i + 1..] == b[i + 1..]
            || (i + 1 < a.len() && a[i] == b[i + 1] && a[i + 1] == b[i] && a[i + 2..] == b[i + 2..])
    } else if a.len() < b.len() {
        a[i..] == b[i + 1..]
    } else {
        a[i + 1..] == b[i..]
    }
}

fn text_score(text: &str, token: &str) -> Option<i32> {
    if text == token {
        return Some(1000);
    }
    if text.starts_with(token) {
        return Some(900);
    }
    if let Some(position) = text.find(token) {
        return Some(800 - position.min(80) as i32);
    }
    let query: Vec<_> = token.chars().collect();
    let mut matched = 0;
    let mut first = 0;
    for (position, ch) in text.chars().enumerate() {
        if matched < query.len() && ch == query[matched] {
            if matched == 0 {
                first = position;
            }
            matched += 1;
            if matched == query.len() {
                let gaps = position - first + 1 - query.len();
                if gaps <= query.len() * 3 + 4 {
                    return Some(600 - (gaps * 8 + first.min(40)) as i32);
                }
                break;
            }
        }
    }
    if token.is_ascii() && (4..=32).contains(&token.len()) {
        if text
            .split(|c: char| !c.is_alphanumeric())
            .take(128)
            .any(|word| word.is_ascii() && one_edit(token.as_bytes(), word.as_bytes()))
        {
            return Some(400);
        }
    }
    None
}

fn query_tokens(query: &str) -> Option<Vec<String>> {
    let query = normalized(query);
    if query.chars().count() > 128 {
        return None;
    }
    let tokens: Vec<_> = query
        .split_whitespace()
        .map(compact)
        .filter(|s| !s.is_empty())
        .take(17)
        .collect();
    (tokens.len() <= 16).then_some(tokens)
}

pub fn score(query: &str, fields: &[(&str, i32)]) -> Option<i32> {
    let tokens = query_tokens(query)?;
    if tokens.is_empty() {
        return Some(0);
    }
    let mut best = vec![None; tokens.len()];
    for (field, weight) in fields {
        let field = normalized(field);
        let original = compact(&field);
        let variants = if field.chars().any(|c| c.to_pinyin_multi().is_some()) {
            pronunciations(&original)
        } else {
            Vec::new()
        };
        for (i, token) in tokens.iter().enumerate() {
            let raw = text_score(&original, token).or_else(|| text_score(&field, token));
            let pinyin = variants
                .iter()
                .flat_map(|variant| {
                    [
                        text_score(&variant.full, token).map(|n| n - 50),
                        text_score(&variant.initials, token).map(|n| n - 90),
                    ]
                })
                .flatten()
                .max();
            if let Some(value) = raw.into_iter().chain(pinyin).max() {
                best[i] = Some(best[i].map_or(value + weight, |old: i32| old.max(value + weight)));
            }
        }
    }
    best.into_iter()
        .try_fold(0, |sum, n| n.map(|value| sum + value))
}

fn match_positions(text: &str, token: &str) -> Option<Vec<usize>> {
    if let Some(byte) = text.find(token) {
        let start = text[..byte].chars().count();
        return Some((start..start + token.chars().count()).collect());
    }
    let mut wanted = token.chars().peekable();
    let mut positions = Vec::new();
    for (index, ch) in text.chars().enumerate() {
        if wanted.peek() == Some(&ch) {
            positions.push(index);
            wanted.next();
        }
        if wanted.peek().is_none() {
            break;
        }
    }
    if wanted.peek().is_none()
        && !positions.is_empty()
        && positions.last().unwrap() - positions[0] + 1 - positions.len() <= positions.len() * 3 + 4
    {
        return Some(positions);
    }
    if token.is_ascii() && (4..=32).contains(&token.len()) {
        let mut offset = 0;
        for word in text
            .split_inclusive(|c: char| !c.is_alphanumeric())
            .take(128)
        {
            let trimmed = word.trim_end_matches(|c: char| !c.is_alphanumeric());
            if trimmed.is_ascii() && one_edit(token.as_bytes(), trimmed.as_bytes()) {
                return Some((offset..offset + trimmed.chars().count()).collect());
            }
            offset += word.chars().count();
        }
    }
    None
}

/// Original visible characters with match spans; never accepts credential secrets.
pub fn highlights(query: &str, text: &str) -> Vec<(String, bool)> {
    let Some(tokens) = query_tokens(query).filter(|tokens| !tokens.is_empty()) else {
        return vec![(text.to_string(), false)];
    };
    let chars: Vec<_> = text.chars().collect();
    let mut marks = vec![false; chars.len()];
    let mut original = String::new();
    let mut origin = Vec::new();
    let mut raw = String::new();
    let mut raw_origin = Vec::new();
    for (position, ch) in chars.iter().enumerate() {
        for folded in normalized(&ch.to_string()).chars() {
            raw.push(folded);
            raw_origin.push(position);
            if folded.is_alphanumeric() {
                original.push(folded);
                origin.push(position);
            }
        }
    }
    let variants = if original.chars().any(|c| c.to_pinyin_multi().is_some()) {
        pronunciations(&original)
    } else {
        Vec::new()
    };
    let identity: Vec<_> = (0..origin.len()).collect();
    let raw_identity: Vec<_> = (0..raw_origin.len()).collect();
    for token in tokens {
        let mut candidates = vec![
            (&original, identity.as_slice(), origin.as_slice(), 0),
            (&raw, raw_identity.as_slice(), raw_origin.as_slice(), 0),
        ];
        for variant in &variants {
            candidates.push((&variant.full, &variant.full_positions, &origin, 50));
            candidates.push((&variant.initials, &variant.initial_positions, &origin, 90));
        }
        if let Some((_, positions, mapping, source)) = candidates
            .into_iter()
            .filter_map(|(text, mapping, source, penalty)| {
                text_score(text, &token)
                    .zip(match_positions(text, &token))
                    .map(|(score, positions)| (score - penalty, positions, mapping, source))
            })
            .max_by_key(|(score, _, _, _)| *score)
        {
            for position in positions {
                if let Some(index) = mapping.get(position).and_then(|p| source.get(*p)) {
                    marks[*index] = true;
                }
            }
        }
    }
    let mut spans: Vec<(String, bool)> = Vec::new();
    for (ch, hit) in chars.into_iter().zip(marks) {
        if spans.last().is_none_or(|(_, marked)| *marked != hit) {
            spans.push((String::new(), hit));
        }
        spans.last_mut().unwrap().0.push(ch);
    }
    spans
}

pub fn highlighted_markdown(query: &str, text: &str) -> String {
    let mut result = String::new();
    for (part, hit) in highlights(query, text) {
        if hit {
            result.push_str("**");
        }
        for ch in part.chars() {
            if "\\*_`[]()<>!#|+-.~&={}:".contains(ch) {
                result.push('\\');
            }
            result.push(ch);
        }
        if hit {
            result.push_str("**");
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn highlight_maps_pinyin_and_initials_back_to_chinese_and_escapes_markup() {
        assert_eq!(
            highlights("yx", "企业邮箱"),
            vec![("企业".into(), false), ("邮箱".into(), true)]
        );
        assert_eq!(
            highlights("youxiang", "企业邮箱"),
            highlights("yx", "企业邮箱")
        );
        assert!(highlights("yinhang", "银行重庆").contains(&("银行".into(), true)));
        assert_eq!(
            highlights("atrsut", "aTrust"),
            vec![("aTrust".into(), true)]
        );
        assert_eq!(
            highlights("ＶＰＮ", "Work VPN"),
            vec![("Work ".into(), false), ("VPN".into(), true)]
        );
        assert_eq!(
            highlighted_markdown("", "[name](url)*"),
            "\\[name\\]\\(url\\)\\*"
        );
        assert_eq!(
            highlights("usre", "demo.user"),
            vec![("demo.".into(), false), ("user".into(), true)]
        );
        assert_eq!(
            highlights("chorme", "Chrome 示例"),
            vec![("Chrome".into(), true), (" 示例".into(), false)]
        );
        assert_eq!(highlights("unknown", "邮箱"), vec![("邮箱".into(), false)]);
    }
    #[test]
    fn searches_chinese_pinyin_initials_width_typos_and_cross_fields() {
        let fields = [("企业邮箱", 120), ("工作 VPN", 90), ("demo.user", 100)];
        for query in ["邮箱", "qiye youxiang", "qyyx", "yx", "ＶＰＮ", "demo VPN"] {
            assert!(score(query, &fields).is_some(), "{query}");
        }
        assert!(score("yinhang chongqing", &[("银行重庆", 120)]).is_some());
        assert!(score("atrsut", &[("aTrust", 120)]).is_some());
        assert!(score("xyzq", &fields).is_none());
        assert!(score(&"a".repeat(129), &fields).is_none());
        assert!(score("yx", &[(&format!("{}邮箱", "工作".repeat(2000)), 120)]).is_some());
        assert!(
            score("atrust", &[("aTrust", 120)]) > score("atrust", &[("a---t--r--u--s--t", 90)])
        );
        assert!(score("vpn", &[("VPN", 120)]) > score("vpn", &[("VPN", 90)]));
    }
}
