use crate::diagnostic::Diagnostic;

pub const SEQUENCE_LIVE_TYPE_PREFIX: &str = "__meld_sequence_live_type_";
pub const SEQUENCE_LITERAL_PREFIX: &str = "__meld_sequence_literal$";
pub const SEQUENCE_INDEX_SEGMENT_PREFIX: &str = "__meld_sequence_index_";

pub fn decode_sequence_live_type(name: &str) -> Option<&str> {
    name.strip_prefix(SEQUENCE_LIVE_TYPE_PREFIX)
}

pub fn decode_sequence_literal(value: &str) -> Option<Vec<String>> {
    let rest = value.strip_prefix(SEQUENCE_LITERAL_PREFIX)?;
    if rest.is_empty() {
        return Some(Vec::new());
    }
    Some(rest.split('|').map(str::to_string).collect())
}

pub fn decode_sequence_index_segment(segment: &str) -> Option<usize> {
    segment
        .strip_prefix(SEQUENCE_INDEX_SEGMENT_PREFIX)?
        .parse()
        .ok()
}

pub fn preprocess(source: &str) -> Result<String, Vec<Diagnostic>> {
    let chars: Vec<char> = source.chars().collect();
    let mut output = String::with_capacity(source.len());
    let mut errors = Vec::new();
    let mut index = 0;
    let mut line = 1;
    let mut column = 1;
    let mut last_significant = None;

    while index < chars.len() {
        let ch = chars[index];

        if ch == '"' {
            copy_string(&chars, &mut index, &mut line, &mut column, &mut output);
            last_significant = Some('"');
            continue;
        }

        if ch == '/' && chars.get(index + 1) == Some(&'/') {
            while index < chars.len() && chars[index] != '\n' {
                push_source_char(
                    chars[index],
                    &mut index,
                    &mut line,
                    &mut column,
                    &mut output,
                );
            }
            continue;
        }

        if ch == '[' {
            let start_line = line;
            let start_column = column;
            let Some(close) = find_closing_bracket(&chars, index + 1) else {
                errors.push(Diagnostic::new(
                    "unterminated ordered-sequence surface",
                    start_line,
                    start_column,
                ));
                break;
            };

            let inner: String = chars[index + 1..close].iter().collect();
            let trimmed = inner.trim();
            let replacement = if !trimmed.is_empty()
                && trimmed.chars().all(|ch| ch.is_ascii_digit())
            {
                format!(".{SEQUENCE_INDEX_SEGMENT_PREFIX}{trimmed}")
            } else if last_significant == Some(':') {
                match parse_live_element(trimmed) {
                    Some(model) => format!("{SEQUENCE_LIVE_TYPE_PREFIX}{model}"),
                    None => {
                        errors.push(Diagnostic::new(
                            "bootstrap ordered-sequence types currently require '[live <StateModel>]'",
                            start_line,
                            start_column,
                        ));
                        String::new()
                    }
                }
            } else if let Some(targets) = parse_live_literal(trimmed) {
                format!("\"{SEQUENCE_LITERAL_PREFIX}{}\"", targets.join("|"))
            } else if can_precede_runtime_index(last_significant) {
                // Preserve non-literal brackets only where an index can
                // syntactically follow an existing primary expression. This is
                // the runtime-index-expression path; semantic validation happens
                // after the index expression has structured AST representation.
                format!("[{inner}]")
            } else {
                errors.push(Diagnostic::new(
                    "bootstrap ordered-sequence literals currently contain only 'live <state-binding>' elements",
                    start_line,
                    start_column,
                ));
                String::new()
            };

            output.push_str(&replacement);
            while index <= close {
                advance_source_position(chars[index], &mut line, &mut column);
                index += 1;
            }
            last_significant = Some(']');
            continue;
        }

        output.push(ch);
        advance_source_position(ch, &mut line, &mut column);
        index += 1;
        if !ch.is_whitespace() {
            last_significant = Some(ch);
        }
    }

    if errors.is_empty() {
        Ok(output)
    } else {
        Err(errors)
    }
}

fn can_precede_runtime_index(last_significant: Option<char>) -> bool {
    matches!(
        last_significant,
        Some(ch) if ch == '_' || ch.is_ascii_alphanumeric() || ch == ')' || ch == ']'
    )
}

fn parse_live_literal(text: &str) -> Option<Vec<String>> {
    if text.is_empty() {
        return Some(Vec::new());
    }
    text.split(',')
        .map(|part| parse_live_element(part.trim()))
        .collect()
}

fn parse_live_element(text: &str) -> Option<String> {
    let mut parts = text.split_whitespace();
    if parts.next()? != "live" {
        return None;
    }
    let name = parts.next()?;
    if parts.next().is_some() || !valid_identifier(name) {
        return None;
    }
    Some(name.to_string())
}

fn valid_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn find_closing_bracket(chars: &[char], mut index: usize) -> Option<usize> {
    while index < chars.len() {
        match chars[index] {
            ']' => return Some(index),
            '[' => return None,
            _ => index += 1,
        }
    }
    None
}

fn copy_string(
    chars: &[char],
    index: &mut usize,
    line: &mut usize,
    column: &mut usize,
    output: &mut String,
) {
    let mut escaped = false;
    let mut opening = true;
    while *index < chars.len() {
        let ch = chars[*index];
        output.push(ch);
        advance_source_position(ch, line, column);
        *index += 1;

        if opening {
            opening = false;
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            break;
        }
    }
}

fn push_source_char(
    ch: char,
    index: &mut usize,
    line: &mut usize,
    column: &mut usize,
    output: &mut String,
) {
    output.push(ch);
    advance_source_position(ch, line, column);
    *index += 1;
}

fn advance_source_position(ch: char, line: &mut usize, column: &mut usize) {
    if ch == '\n' {
        *line += 1;
        *column = 1;
    } else {
        *column += 1;
    }
}
