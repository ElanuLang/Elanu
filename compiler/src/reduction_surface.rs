use crate::ast::{Declaration, Expr};
use crate::diagnostic::Diagnostic;
use crate::{lexer, parser::Parser, sequence_surface};

pub const REDUCTION_PREFIX: &str = "__meld_reduce$";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReductionSpec {
    pub source: String,
    pub element_model: Option<String>,
    pub initial_source: String,
    pub accumulator: String,
    pub element: String,
    pub step_source: String,
}

pub fn encode_reduction(spec: &ReductionSpec) -> String {
    let fields = [
        spec.source.as_str(),
        spec.element_model.as_deref().unwrap_or(""),
        spec.initial_source.as_str(),
        spec.accumulator.as_str(),
        spec.element.as_str(),
        spec.step_source.as_str(),
    ];
    let encoded = fields
        .into_iter()
        .map(hex_encode)
        .collect::<Vec<_>>()
        .join("$");
    format!("{REDUCTION_PREFIX}{encoded}")
}

pub fn decode_reduction(value: &str) -> Option<ReductionSpec> {
    let rest = value.strip_prefix(REDUCTION_PREFIX)?;
    let parts = rest.split('$').collect::<Vec<_>>();
    if parts.len() != 6 {
        return None;
    }

    let source = hex_decode(parts[0])?;
    let element_model = match hex_decode(parts[1])? {
        value if value.is_empty() => None,
        value => Some(value),
    };

    Some(ReductionSpec {
        source,
        element_model,
        initial_source: hex_decode(parts[2])?,
        accumulator: hex_decode(parts[3])?,
        element: hex_decode(parts[4])?,
        step_source: hex_decode(parts[5])?,
    })
}

/// Bootstrap surface preprocessor for the provisional reduction spelling:
///
/// `reduce source from initial as (accumulator, element) { step }`
///
/// The replacement is a source-inaccessible String marker consumed by later
/// lowering passes. This is deliberately an experiment, not permanent parser
/// architecture.
pub fn preprocess(source: &str) -> Result<String, Vec<Diagnostic>> {
    let mut output = String::with_capacity(source.len());
    let mut index = 0;

    while index < source.len() {
        if source.as_bytes()[index] == b'"' {
            let end = scan_string(source, index);
            output.push_str(&source[index..end]);
            index = end;
            continue;
        }

        if source[index..].starts_with("//") {
            let end = source[index..]
                .find('\n')
                .map(|offset| index + offset)
                .unwrap_or(source.len());
            output.push_str(&source[index..end]);
            index = end;
            continue;
        }

        if keyword_at(source, index, "reduce") {
            let start = index;
            match parse_reduction_surface(source, index) {
                Ok((end, spec)) => {
                    output.push('"');
                    output.push_str(&encode_reduction(&spec));
                    output.push('"');

                    // Preserve line count for diagnostics below a multi-line
                    // reduction. Column fidelity is intentionally not promised
                    // by this bootstrap preprocessor.
                    for _ in source[start..end].bytes().filter(|byte| *byte == b'\n') {
                        output.push('\n');
                    }
                    index = end;
                    continue;
                }
                Err(_) => {
                    // `reduce` is provisional contextual syntax, not a selected
                    // reserved word. If the full surface does not parse, leave
                    // the identifier alone for the ordinary lexer/parser.
                    output.push_str("reduce");
                    index += "reduce".len();
                    continue;
                }
            }
        }

        let ch = source[index..]
            .chars()
            .next()
            .expect("index should remain on a character boundary");
        output.push(ch);
        index += ch.len_utf8();
    }

    Ok(output)
}

pub fn parse_expression_fragment(source: &str) -> Result<Expr, Vec<Diagnostic>> {
    let synthetic = format!("derived __meld_reduce_fragment = {}\n", source.trim());
    let synthetic = sequence_surface::preprocess(&synthetic)?;
    let tokens = lexer::lex(&synthetic)?;
    let program = Parser::new(tokens).parse_program()?;

    match program.declarations.as_slice() {
        [Declaration::Derived(derived)] => Ok(derived.expression.clone()),
        _ => Err(vec![Diagnostic::new(
            "internal reduction fragment did not parse as one expression",
            1,
            1,
        )]),
    }
}

fn parse_reduction_surface(source: &str, start: usize) -> Result<(usize, ReductionSpec), String> {
    let mut index = start + "reduce".len();
    index = skip_whitespace(source, index);

    let (sequence, next) = parse_name_path(source, index)
        .ok_or_else(|| "expected sequence name after 'reduce'".to_string())?;
    index = next;

    index = expect_keyword(source, index, "from")?;
    let initial_start = index;
    let as_index = find_top_level_keyword(source, index, "as")
        .ok_or_else(|| "expected 'as' after reduction initial value".to_string())?;
    let initial_source = source[initial_start..as_index].trim().to_string();
    if initial_source.is_empty() {
        return Err("reduction requires an initial accumulator expression".to_string());
    }

    index = as_index + "as".len();
    index = skip_whitespace(source, index);
    index = expect_byte(source, index, b'(', "expected '(' after reduction 'as'")?;
    index = skip_whitespace(source, index);

    let (accumulator, next) = parse_identifier(source, index)
        .ok_or_else(|| "expected accumulator binding in reduction".to_string())?;
    index = skip_whitespace(source, next);
    index = expect_byte(
        source,
        index,
        b',',
        "expected ',' between reduction bindings",
    )?;
    index = skip_whitespace(source, index);

    let (element, next) = parse_identifier(source, index)
        .ok_or_else(|| "expected element binding in reduction".to_string())?;
    index = skip_whitespace(source, next);
    index = expect_byte(source, index, b')', "expected ')' after reduction bindings")?;
    index = skip_whitespace(source, index);
    index = expect_byte(source, index, b'{', "expected '{' before reduction step")?;

    let step_start = index;
    let close = find_matching_brace(source, index - 1)
        .ok_or_else(|| "unterminated reduction step block".to_string())?;
    let step_source = source[step_start..close].trim().to_string();
    if step_source.is_empty() {
        return Err("reduction requires a step expression".to_string());
    }

    Ok((
        close + 1,
        ReductionSpec {
            source: sequence,
            element_model: None,
            initial_source,
            accumulator,
            element,
            step_source,
        },
    ))
}

fn find_top_level_keyword(source: &str, mut index: usize, keyword: &str) -> Option<usize> {
    let mut paren_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut bracket_depth = 0usize;

    while index < source.len() {
        if source.as_bytes()[index] == b'"' {
            index = scan_string(source, index);
            continue;
        }
        if source[index..].starts_with("//") {
            index = source[index..]
                .find('\n')
                .map(|offset| index + offset)
                .unwrap_or(source.len());
            continue;
        }

        match source.as_bytes()[index] {
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.saturating_sub(1),
            b'{' => brace_depth += 1,
            b'}' => brace_depth = brace_depth.saturating_sub(1),
            b'[' => bracket_depth += 1,
            b']' => bracket_depth = bracket_depth.saturating_sub(1),
            _ => {}
        }

        if paren_depth == 0
            && brace_depth == 0
            && bracket_depth == 0
            && keyword_at(source, index, keyword)
        {
            return Some(index);
        }

        index += source[index..].chars().next()?.len_utf8();
    }

    None
}

fn find_matching_brace(source: &str, open: usize) -> Option<usize> {
    let mut index = open + 1;
    let mut depth = 1usize;

    while index < source.len() {
        if source.as_bytes()[index] == b'"' {
            index = scan_string(source, index);
            continue;
        }
        if source[index..].starts_with("//") {
            index = source[index..]
                .find('\n')
                .map(|offset| index + offset)
                .unwrap_or(source.len());
            continue;
        }

        match source.as_bytes()[index] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += source[index..].chars().next()?.len_utf8();
    }

    None
}

fn parse_name_path(source: &str, index: usize) -> Option<(String, usize)> {
    let (first, mut index) = parse_identifier(source, index)?;
    let mut value = first;

    loop {
        let after_space = skip_whitespace(source, index);
        if source.as_bytes().get(after_space) != Some(&b'.') {
            return Some((value, index));
        }
        let member_start = skip_whitespace(source, after_space + 1);
        let (member, next) = parse_identifier(source, member_start)?;
        value.push('.');
        value.push_str(&member);
        index = next;
    }
}

fn parse_identifier(source: &str, index: usize) -> Option<(String, usize)> {
    let first = *source.as_bytes().get(index)?;
    if !is_ident_start(first) {
        return None;
    }
    let mut end = index + 1;
    while source
        .as_bytes()
        .get(end)
        .copied()
        .is_some_and(is_ident_continue)
    {
        end += 1;
    }
    Some((source[index..end].to_string(), end))
}

fn expect_keyword(source: &str, index: usize, keyword: &str) -> Result<usize, String> {
    let index = skip_whitespace(source, index);
    if keyword_at(source, index, keyword) {
        Ok(index + keyword.len())
    } else {
        Err(format!("expected '{keyword}' in reduction"))
    }
}

fn expect_byte(source: &str, index: usize, expected: u8, message: &str) -> Result<usize, String> {
    if source.as_bytes().get(index) == Some(&expected) {
        Ok(index + 1)
    } else {
        Err(message.to_string())
    }
}

fn skip_whitespace(source: &str, mut index: usize) -> usize {
    while let Some(ch) = source[index..].chars().next() {
        if !ch.is_whitespace() {
            break;
        }
        index += ch.len_utf8();
        if index >= source.len() {
            break;
        }
    }
    index
}

fn scan_string(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    let mut index = start + 1;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        index += 1;
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            break;
        }
    }
    index
}

fn keyword_at(source: &str, index: usize, keyword: &str) -> bool {
    if !source[index..].starts_with(keyword) {
        return false;
    }
    let before_ok = index == 0 || !is_ident_continue(source.as_bytes()[index - 1]);
    let after = index + keyword.len();
    let after_ok = after >= source.len() || !is_ident_continue(source.as_bytes()[after]);
    before_ok && after_ok
}

fn is_ident_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn is_ident_continue(byte: u8) -> bool {
    is_ident_start(byte) || byte.is_ascii_digit()
}

fn hex_encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hex_decode(value: &str) -> Option<String> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let bytes = (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}
