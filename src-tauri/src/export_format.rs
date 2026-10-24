use serde_json::Value;
use std::borrow::Cow;
use std::io::{self, Write};

pub(crate) struct ExportWriter<W> {
    output: W,
    json: bool,
    first_session: bool,
    first_entry: bool,
    first_tool: bool,
    first_section: bool,
}

fn text(value: &Value) -> Cow<'_, str> {
    match value {
        Value::Null => Cow::Borrowed(""),
        Value::String(value) => Cow::Borrowed(value),
        value => Cow::Owned(value.to_string()),
    }
}

fn escaped(value: &Value) -> String {
    let mut output = String::new();
    let mut newline = false;
    for character in text(value).chars() {
        if matches!(character, '\r' | '\n') {
            if !newline {
                output.push(' ');
            }
            newline = true;
        } else {
            newline = false;
            if "\\`*_{}[]<>#|".contains(character) {
                output.push('\\');
            }
            output.push(character);
        }
    }
    output
}

fn pretty(value: &Value) -> Cow<'_, str> {
    let raw = text(value);
    serde_json::from_str::<Value>(&raw)
        .and_then(|value| serde_json::to_string_pretty(&value))
        .map(Cow::Owned)
        .unwrap_or(raw)
}

fn json(output: &mut impl Write, value: &Value) -> io::Result<()> {
    serde_json::to_writer(output, value).map_err(io::Error::other)
}

fn code(output: &mut impl Write, value: &str, language: &str) -> io::Result<()> {
    let mut longest = 0;
    let mut current = 0;
    for byte in value.bytes() {
        current = if byte == b'`' { current + 1 } else { 0 };
        longest = longest.max(current);
    }
    let fence = "`".repeat(3.max(longest + 1));
    write!(output, "{fence}{language}\n{value}\n{fence}")
}

fn present(value: &Value) -> bool {
    !value.is_null() && value.as_str() != Some("")
}

impl<W: Write> ExportWriter<W> {
    pub(crate) fn new(output: W, format: &str) -> Self {
        Self {
            output,
            json: format == "json",
            first_session: true,
            first_entry: true,
            first_tool: true,
            first_section: true,
        }
    }

    fn section(&mut self) -> io::Result<()> {
        if !self.first_section {
            self.output.write_all(b"\n\n")?;
        }
        self.first_section = false;
        Ok(())
    }

    pub(crate) fn begin(&mut self, scope: &str, exported_at: &str, count: usize) -> io::Result<()> {
        if self.json {
            self.output
                .write_all(b"{\"schemaVersion\":1,\"exportedAt\":")?;
            json(&mut self.output, &Value::from(exported_at))?;
            self.output.write_all(b",\"scope\":")?;
            json(&mut self.output, &Value::from(scope))?;
            self.output.write_all(b",\"sessions\":[")
        } else {
            self.section()?;
            write!(self.output, "# {} export", crate::brand::NAME)?;
            self.section()?;
            write!(
                self.output,
                "Exported: {}",
                escaped(&Value::from(exported_at))
            )?;
            self.section()?;
            write!(self.output, "{count} session(s)")
        }
    }

    pub(crate) fn session(&mut self, session: &Value, stats: Option<&Value>) -> io::Result<()> {
        self.first_entry = true;
        self.first_tool = true;
        if self.json {
            if !self.first_session {
                self.output.write_all(b",")?;
            }
            self.first_session = false;
            self.output.write_all(b"{\"session\":")?;
            json(&mut self.output, session)?;
            if let Some(stats) = stats {
                self.output.write_all(b",\"stats\":")?;
                json(&mut self.output, stats)?;
            }
            self.output.write_all(b",\"entries\":[")
        } else {
            self.section()?;
            writeln!(
                self.output,
                "---\n\n## {}",
                if present(&session["title"]) {
                    escaped(&session["title"])
                } else {
                    "Untitled session".into()
                }
            )?;
            // The heading's final newline belongs to the section separator.
            self.output.write_all(b"\n")?;
            let fallback = |key: &str, alternative: &str| {
                if present(&session[key]) {
                    escaped(&session[key])
                } else {
                    alternative.to_string()
                }
            };
            let agent = if present(&session["product"]) {
                &session["product"]
            } else {
                &session["agent"]
            };
            let workspace = if present(&session["project"]) {
                &session["project"]
            } else {
                &session["cwd"]
            };
            let tags = session["tags"]
                .as_array()
                .map(|tags| tags.iter().map(escaped).collect::<Vec<_>>().join(", "))
                .filter(|tags| !tags.is_empty())
                .unwrap_or_else(|| "None".into());
            write!(self.output, "- Agent: {}\n- Workspace: {}\n- Started: {}\n- Last activity: {}\n- Starred: {}\n- Tags: {}",
                escaped(agent), if present(workspace) { escaped(workspace) } else { "Unknown".into() },
                fallback("started_at", "Unknown"), fallback("last_event_at", "Unknown"),
                if session["starred"].as_bool() == Some(true) { "Yes" } else { "No" }, tags)?;
            for (key, label) in [("branch", "Branch"), ("model", "Model")] {
                if present(&session[key]) {
                    write!(self.output, "\n- {label}: {}", escaped(&session[key]))?;
                }
            }
            if let Some(stats) = stats {
                self.section()?;
                self.output.write_all(b"### Session stats")?;
                self.section()?;
                if let Some(stats) = stats.as_object() {
                    for (index, (key, value)) in stats.iter().enumerate() {
                        if index > 0 {
                            self.output.write_all(b"\n")?;
                        }
                        write!(
                            self.output,
                            "- {}: {}",
                            escaped(&Value::from(key.replace('_', " "))),
                            escaped(value)
                        )?;
                    }
                }
            }
            self.section()?;
            self.output.write_all(b"### Transcript")
        }
    }

    pub(crate) fn entry(&mut self, entry: &Value) -> io::Result<()> {
        if self.json {
            if !self.first_entry {
                self.output.write_all(b",")?;
            }
            self.first_entry = false;
            return json(&mut self.output, entry);
        }
        self.section()?;
        let role = if present(&entry["role"]) {
            &entry["role"]
        } else if present(&entry["kind"]) {
            &entry["kind"]
        } else {
            &Value::Null
        };
        write!(
            self.output,
            "#### {}",
            if present(role) {
                escaped(role)
            } else {
                "Event".into()
            }
        )?;
        if present(&entry["occurred_at"]) {
            write!(self.output, " · {}", escaped(&entry["occurred_at"]))?;
        }
        let mut first = true;
        if let Some(parts) = entry["parts"].as_array() {
            for part in parts {
                if part["kind"] == "text" && !present(&part["text"]) {
                    continue;
                }
                self.section()?;
                first = false;
                if part["kind"] == "text" {
                    self.output.write_all(text(&part["text"]).as_bytes())?;
                } else {
                    write!(
                        self.output,
                        "**{}**",
                        if present(&part["kind"]) {
                            escaped(&part["kind"])
                        } else {
                            "Event".into()
                        }
                    )?;
                    for (key, language, format) in
                        [("text", "text", false), ("data_json", "json", true)]
                    {
                        if present(&part[key]) {
                            self.output.write_all(b"\n\n")?;
                            code(
                                &mut self.output,
                                &if format {
                                    pretty(&part[key])
                                } else {
                                    text(&part[key])
                                },
                                language,
                            )?;
                        }
                    }
                }
            }
        }
        if let Some(tools) = entry["tools"].as_array() {
            for tool in tools {
                self.section()?;
                self.tool(tool)?;
                first = false;
            }
        }
        if first {
            self.section()?;
            if present(&entry["search_text"]) {
                self.output
                    .write_all(text(&entry["search_text"]).as_bytes())?;
            } else if present(&entry["payload_json"]) {
                code(&mut self.output, &pretty(&entry["payload_json"]), "json")?;
            } else {
                write!(
                    self.output,
                    "_{}_",
                    if present(&entry["kind"]) {
                        escaped(&entry["kind"])
                    } else {
                        "Event".into()
                    }
                )?;
            }
        }
        Ok(())
    }

    fn tool(&mut self, tool: &Value) -> io::Result<()> {
        write!(
            self.output,
            "**{}**",
            if present(&tool["tool_name"]) {
                escaped(&tool["tool_name"])
            } else {
                "Tool".into()
            }
        )?;
        if present(&tool["status"]) {
            write!(self.output, " · {}", escaped(&tool["status"]))?;
        }
        for (key, language, format) in [
            ("command_text", "sh", false),
            ("arguments_json", "json", true),
            ("result_text", "text", false),
        ] {
            if present(&tool[key]) {
                self.output.write_all(b"\n\n")?;
                code(
                    &mut self.output,
                    &if format {
                        pretty(&tool[key])
                    } else {
                        text(&tool[key])
                    },
                    language,
                )?;
            }
        }
        Ok(())
    }

    pub(crate) fn unlinked_tools(&mut self) -> io::Result<()> {
        if self.json {
            self.output.write_all(b"],\"unlinkedTools\":[")?;
        }
        Ok(())
    }

    pub(crate) fn unlinked_tool(&mut self, tool: &Value) -> io::Result<()> {
        if self.json {
            if !self.first_tool {
                self.output.write_all(b",")?;
            }
            self.first_tool = false;
            json(&mut self.output, tool)
        } else {
            if self.first_tool {
                self.first_tool = false;
                self.section()?;
                self.output.write_all(b"### Additional tool activity")?;
            }
            self.section()?;
            self.tool(tool)
        }
    }

    pub(crate) fn end_session(&mut self) -> io::Result<()> {
        if self.json {
            self.output.write_all(b"]}")?;
        }
        Ok(())
    }

    pub(crate) fn finish(mut self) -> io::Result<W> {
        self.output
            .write_all(if self.json { b"]}\n" } else { b"\n" })?;
        Ok(self.output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn markdown_preserves_sections_unicode_and_safe_code_fences() {
        let mut output = ExportWriter::new(Vec::new(), "markdown");
        output.begin("session", "2026-10-24", 1).unwrap();
        output.session(&json!({"title":"A_* 🦀","agent":"Codex","cwd":"/tmp","starred":true,"tags":["tag"]}),None).unwrap();
        output
            .entry(&json!({"role":"user","parts":[{"kind":"text","text":"Hello\nworld"}]}))
            .unwrap();
        output.entry(&json!({"kind":"event","parts":[],"tools":[{"tool_name":"shell","status":"completed","command_text":"echo ```","result_text":"result"}]})).unwrap();
        output.unlinked_tools().unwrap();
        output
            .unlinked_tool(&json!({"tool_name":"Other","arguments_json":"{\"a\":1}"}))
            .unwrap();
        output.end_session().unwrap();
        let actual = String::from_utf8(output.finish().unwrap()).unwrap();
        let expected=format!("# {} export\n\nExported: 2026-10-24\n\n1 session(s)\n\n---\n\n## A\\_\\* 🦀\n\n- Agent: Codex\n- Workspace: /tmp\n- Started: Unknown\n- Last activity: Unknown\n- Starred: Yes\n- Tags: tag\n\n### Transcript\n\n#### user\n\nHello\nworld\n\n#### event\n\n**shell** · completed\n\n````sh\necho ```\n````\n\n```text\nresult\n```\n\n### Additional tool activity\n\n**Other**\n\n```json\n{{\n  \"a\": 1\n}}\n```\n",crate::brand::NAME);
        assert_eq!(actual, expected);
    }

    #[test]
    fn empty_exports_and_missing_titles_remain_valid() {
        for format in ["json", "markdown"] {
            let mut output = ExportWriter::new(Vec::new(), format);
            output.begin("library", "now", 0).unwrap();
            let bytes = output.finish().unwrap();
            if format == "json" {
                assert_eq!(
                    serde_json::from_slice::<Value>(&bytes).unwrap()["sessions"],
                    json!([])
                );
            } else {
                assert!(String::from_utf8(bytes)
                    .unwrap()
                    .ends_with("0 session(s)\n"));
            }
        }
    }
}
