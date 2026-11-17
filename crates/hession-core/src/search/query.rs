//! Query grammar: whitespace AND binds more tightly than explicit uppercase OR.
//! No raw SQL/FTS input, regexes or Boolean groups. Positions are UTF-8 byte offsets.
use serde::{Deserialize, Serialize};

pub const MAX_QUERY_BYTES: usize = 4096;
pub const MAX_TERMS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryError {
    pub position: usize,
    pub message: String,
}
impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at byte {}", self.message, self.position)
    }
}
impl std::error::Error for QueryError {}
fn error(position: usize, message: &str) -> QueryError {
    QueryError {
        position,
        message: message.into(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Field {
    Agent,
    Project,
    File,
    Tool,
    Branch,
    After,
    Before,
    Errors,
    Host,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Term {
    pub field: Option<Field>,
    pub value: String,
    pub excluded: bool,
    pub phrase: bool,
    pub position: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub alternatives: Vec<Vec<Term>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Term(Term),
    Or(usize),
}

pub fn lex(input: &str) -> Result<Vec<Token>, QueryError> {
    if input.len() > MAX_QUERY_BYTES {
        return Err(error(MAX_QUERY_BYTES, "Query exceeds 4096 bytes"));
    }
    let mut tokens = Vec::new();
    let mut pos = 0;
    while pos < input.len() {
        let ch = input[pos..].chars().next().unwrap();
        if ch.is_whitespace() {
            pos += ch.len_utf8();
            continue;
        }
        let start = pos;
        let excluded = input[pos..].starts_with('-');
        if excluded {
            pos += 1;
        }
        if pos == input.len() || input[pos..].starts_with(char::is_whitespace) {
            return Err(error(start, "Exclusion needs a term"));
        }
        // A recognized field is syntax; :: symbols and URI schemes are literal.
        let value_start = pos;
        let end = input[pos..]
            .find(|c: char| c.is_whitespace() || c == '"')
            .map(|n| pos + n)
            .unwrap_or(input.len());
        let mut field = None;
        if let Some(colon) = input[pos..end].find(':') {
            let name = &input[pos..pos + colon];
            let rest = &input[pos + colon + 1..];
            let known_field = matches!(
                name,
                "agent"
                    | "project"
                    | "file"
                    | "tool"
                    | "branch"
                    | "after"
                    | "before"
                    | "has"
                    | "host"
            );
            if !rest.starts_with(':')
                && (known_field
                    || (!rest.starts_with('/')
                        && !rest.starts_with('\\')
                        && !name.contains('/')
                        && !name.contains('\\')))
            {
                field = Some(match name {
                    "agent" => Field::Agent, "project" => Field::Project, "file" => Field::File,
                    "tool" => Field::Tool, "branch" => Field::Branch, "after" => Field::After,
                    "before" => Field::Before, "has" => Field::Errors, "host" => Field::Host,
                    _ => return Err(error(pos, "Unknown field; use agent, project, file, tool, branch, after, before, has or host")),
                });
                pos += colon + 1;
            }
        }
        let phrase = input[pos..].starts_with('"');
        let mut value = String::new();
        if phrase {
            let quote = pos;
            pos += 1;
            let mut closed = false;
            while pos < input.len() {
                let c = input[pos..].chars().next().unwrap();
                pos += c.len_utf8();
                if c == '"' {
                    closed = true;
                    break;
                }
                if c == '\\' && pos < input.len() {
                    let next = input[pos..].chars().next().unwrap();
                    if next == '"' || next == '\\' {
                        value.push(next);
                        pos += next.len_utf8();
                        continue;
                    }
                }
                value.push(c);
            }
            if !closed {
                return Err(error(quote, "Unclosed quoted phrase"));
            }
            if pos < input.len() && !input[pos..].starts_with(char::is_whitespace) {
                return Err(error(pos, "Add whitespace after a quoted phrase"));
            }
        } else {
            while pos < input.len() {
                let c = input[pos..].chars().next().unwrap();
                if c.is_whitespace() {
                    break;
                }
                if c == '"' {
                    return Err(error(pos, "Quotes must enclose the whole value"));
                }
                value.push(c);
                pos += c.len_utf8();
            }
        }
        if !phrase && (value.starts_with('(') || value.starts_with(')')) {
            return Err(error(
                start,
                "Boolean groups are unsupported; quote literal parentheses",
            ));
        }
        if value.is_empty() {
            return Err(error(value_start, "Field or phrase needs a nonempty value"));
        }
        if value.chars().any(char::is_control) {
            return Err(error(start, "Control characters are not allowed"));
        }
        if field.is_none() && !phrase && value == "OR" {
            if excluded {
                return Err(error(start, "OR cannot be excluded"));
            }
            tokens.push(Token::Or(start));
        } else {
            if matches!(field, Some(Field::After | Field::Before)) && !valid_date(&value) {
                return Err(error(
                    value_start,
                    "Dates must be valid UTC calendar dates YYYY-MM-DD",
                ));
            }
            if field == Some(Field::Errors) && value != "errors" {
                return Err(error(value_start, "Only has:errors is supported"));
            }
            tokens.push(Token::Term(Term {
                field,
                value,
                excluded,
                phrase,
                position: start,
            }));
        }
        if tokens.len() > MAX_TERMS * 2 {
            return Err(error(start, "Query exceeds 64 terms"));
        }
    }
    Ok(tokens)
}

pub fn parse(input: &str) -> Result<Query, QueryError> {
    let tokens = lex(input)?;
    let mut alternatives = vec![Vec::new()];
    let mut count = 0;
    let mut last_or = None;
    for token in tokens {
        match token {
            Token::Or(pos) => {
                if alternatives.last().unwrap().is_empty() {
                    return Err(error(pos, "OR needs a term on both sides"));
                }
                alternatives.push(Vec::new());
                last_or = Some(pos);
            }
            Token::Term(term) => {
                count += 1;
                if count > MAX_TERMS {
                    return Err(error(term.position, "Query exceeds 64 terms"));
                }
                alternatives.last_mut().unwrap().push(term);
            }
        }
    }
    if alternatives.len() > 1 && alternatives.last().unwrap().is_empty() {
        return Err(error(last_or.unwrap(), "OR needs a term on both sides"));
    }
    Ok(Query { alternatives })
}

pub(crate) fn valid_date(value: &str) -> bool {
    let b = value.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || b.iter()
            .enumerate()
            .any(|(i, c)| i != 4 && i != 7 && !c.is_ascii_digit())
    {
        return false;
    }
    let y: u32 = value[..4].parse().unwrap();
    let m: u32 = value[5..7].parse().unwrap();
    let d: u32 = value[8..].parse().unwrap();
    let days = match m {
        4 | 6 | 9 | 11 => 30,
        2 if y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)) => 29,
        2 => 28,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    y > 0 && d > 0 && d <= days
}

/// FTS text is always a quoted string, never user-authored FTS operators.
pub fn fts_literal(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
/// Programming punctuation must retain exact symbols, unlike unicode61 tokens.
pub fn is_literal(value: &str) -> bool {
    value.chars().any(|c| {
        !c.is_alphanumeric()
            && !c.is_whitespace()
            && !unicode_normalization::char::is_combining_mark(c)
    }) || !value.chars().any(char::is_alphanumeric)
}
