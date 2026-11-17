use super::{ExportError, ExportOptions};
use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionSummary {
    pub secret_patterns: u64,
    pub path_patterns: u64,
}
struct Patterns {
    private_key: Regex,
    assignment: Regex,
    token: Regex,
    bearer: Regex,
    url_user: Regex,
    unix_path: Regex,
    windows_path: Regex,
    quoted_path: Regex,
    secret_key: Regex,
}
fn patterns() -> &'static Patterns {
    static VALUE: OnceLock<Patterns> = OnceLock::new();
    VALUE.get_or_init(||Patterns{
    private_key:Regex::new(r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|$)").expect("constant regex"),
    assignment:Regex::new(r#"(?i)(\b(?:[a-z][a-z0-9_]*_)?(?:api[_-]?key|access[_-]?token|auth[_-]?token|secret(?:[_-]access)?(?:[_-]?key)?|password|passwd|authorization|token)\b["']?\s*[:=]\s*)(?:"[^"]*"|'[^']*'|[^\s,;}\]]+)"#).expect("constant regex"),
    token:Regex::new(r"\b(?:sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{12,}|github_pat_[A-Za-z0-9_]{12,}|AKIA[0-9A-Z]{16})").expect("constant regex"),
    bearer:Regex::new(r"(?i)\b(Bearer\s+)[A-Za-z0-9_./+=:-]{8,}").expect("constant regex"),
    url_user:Regex::new(r"(?i)\b([a-z][a-z0-9+.-]*://)[^/\s@]+@").expect("constant regex"),
    unix_path:Regex::new(r#"(?m)(^|[\s"'`=(\[{<>])((?:[a-zA-Z0-9_.@-]+:)?/[^\s"'`<>{}\[\]()]+)"#).expect("constant regex"),
    windows_path:Regex::new(r#"(?i)(?:[a-z]:[\\/]|\\\\)[^\s"'`<>{}\[\]()]+"#).expect("constant regex"),
    secret_key:Regex::new(r"(?i)(?:^|[_-])(?:api[_-]?key|access[_-]?token|auth[_-]?token|secret(?:[_-]access)?(?:[_-]?key)?|password|passwd|authorization|token)$").expect("constant regex"),
    quoted_path:Regex::new(r#"["'](/[^"'\r\n]*)["']"#).expect("constant regex"),
})
}

pub(super) struct Sanitizer<'a> {
    pub options: &'a ExportOptions,
    pub summary: RedactionSummary,
}
impl<'a> Sanitizer<'a> {
    pub fn new(options: &'a ExportOptions) -> Self {
        Self {
            options,
            summary: RedactionSummary::default(),
        }
    }
    pub fn text(&mut self, text: &str) -> String {
        let p = patterns();
        let mut result = text.to_owned();
        if self.options.redact {
            for (pattern, replacement) in [
                (&p.private_key, "[REDACTED_PRIVATE_KEY]"),
                (&p.token, "[REDACTED_SECRET]"),
                (&p.bearer, "${1}[REDACTED_SECRET]"),
                (&p.assignment, "${1}[REDACTED_SECRET]"),
                (&p.url_user, "${1}[REDACTED_CREDENTIALS]@"),
            ] {
                self.summary.secret_patterns += pattern.find_iter(&result).count() as u64;
                result = if std::ptr::eq(pattern, &p.assignment) {
                    pattern
                        .replace_all(&result, |capture: &Captures<'_>| {
                            format!(
                                "{}{}",
                                &capture[1],
                                if capture[0].ends_with("[REDACTED_SECRET") {
                                    "[REDACTED_SECRET"
                                } else {
                                    "[REDACTED_SECRET]"
                                }
                            )
                        })
                        .into_owned()
                } else {
                    pattern.replace_all(&result, replacement).into_owned()
                };
            }
        }
        if !self.options.include_absolute_paths {
            self.summary.path_patterns += p.quoted_path.find_iter(&result).count() as u64;
            result = p
                .quoted_path
                .replace_all(&result, "\"[OMITTED_ABSOLUTE_PATH]\"")
                .into_owned();
            let mut count = 0;
            result = p
                .unix_path
                .replace_all(&result, |capture: &Captures<'_>| {
                    if capture[2].starts_with("https://") || capture[2].starts_with("http://") {
                        return capture[0].to_string();
                    }
                    count += 1;
                    format!("{}[OMITTED_ABSOLUTE_PATH]", &capture[1])
                })
                .into_owned();
            self.summary.path_patterns += count;
            self.summary.path_patterns += p.windows_path.find_iter(&result).count() as u64;
            result = p
                .windows_path
                .replace_all(&result, "[OMITTED_ABSOLUTE_PATH]")
                .into_owned();
        }
        result
    }
    pub fn value(&mut self, value: &mut Value) -> Result<(), ExportError> {
        self.visit(value, 0, &mut 0)
    }
    fn visit(
        &mut self,
        value: &mut Value,
        depth: usize,
        nodes: &mut usize,
    ) -> Result<(), ExportError> {
        *nodes += 1;
        if depth > 64 || *nodes > 50_000 {
            return Err(ExportError::Limit("JSON structure"));
        }
        match value {
            Value::String(text) => *text = self.text(text),
            Value::Array(values) => {
                for value in values {
                    self.visit(value, depth + 1, nodes)?;
                }
            }
            Value::Object(values) => {
                let old = std::mem::take(values);
                for (key, mut value) in old {
                    if self.options.redact && patterns().secret_key.is_match(&key) {
                        self.summary.secret_patterns += 1;
                        value = Value::String("[REDACTED_SECRET]".into());
                    } else {
                        self.visit(&mut value, depth + 1, nodes)?;
                    }
                    values.insert(self.text(&key), value);
                }
            }
            _ => (),
        }
        Ok(())
    }
    pub fn structured_text(&mut self, text: &str) -> Result<String, ExportError> {
        if let Ok(mut json) = serde_json::from_str::<Value>(text) {
            if json.is_object() || json.is_array() {
                self.value(&mut json)?;
                return Ok(serde_json::to_string(&json)?);
            }
        }
        Ok(self.text(text))
    }
}
