//! What actions return: structured enough for panels to render and for the LLM
//! to read.

#[derive(Clone, Debug)]
pub struct Column {
    /// Matches a parameter name when a row is used to fill in a row action.
    pub key: &'static str,
    pub title: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<String>>,
    /// Columns kept when there isn't room for all of them (the rest show on
    /// request and in a row's details); empty means all.
    pub essential: Vec<&'static str>,
}

impl Table {
    pub fn new(columns: &[(&'static str, &'static str)]) -> Self {
        Self { columns: columns.iter().map(|&(key, title)| Column { key, title }).collect(), rows: Vec::new(), essential: Vec::new() }
    }

    pub fn essential(mut self, keys: &[&'static str]) -> Self {
        self.essential = keys.to_vec();
        self
    }

    pub fn is_essential(&self, column: usize) -> bool {
        self.essential.is_empty() || self.columns.get(column).is_some_and(|c| self.essential.contains(&c.key))
    }

    pub fn push(&mut self, row: Vec<String>) {
        self.rows.push(row);
    }

    pub fn cell<'a>(&'a self, row: &'a [String], key: &str) -> Option<&'a str> {
        let i = self.columns.iter().position(|c| c.key == key)?;
        row.get(i).map(String::as_str)
    }
}

#[derive(Clone, Debug)]
pub enum Output {
    /// A finished change, described in one line.
    Done(String),
    Text(String),
    Facts(Vec<(String, String)>),
    Table(Table),
}

impl Output {
    /// Plain-text rendering for the LLM, cut to `max_rows` table rows.
    pub fn to_llm_text(&self, max_rows: usize) -> String {
        match self {
            Output::Done(s) | Output::Text(s) => s.clone(),
            Output::Facts(facts) => facts.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n"),
            Output::Table(t) => {
                let mut out = t.columns.iter().map(|c| c.key).collect::<Vec<_>>().join(" | ");
                for row in t.rows.iter().take(max_rows) {
                    out.push('\n');
                    out.push_str(&row.join(" | "));
                }
                if t.rows.len() > max_rows {
                    out.push_str(&format!("\n… {} more rows", t.rows.len() - max_rows));
                }
                if t.rows.is_empty() {
                    out.push_str("\n(no rows)");
                }
                out
            }
        }
    }

    /// One-line summary for conversation entries.
    pub fn headline(&self) -> String {
        match self {
            Output::Done(s) => s.clone(),
            Output::Text(s) => s.lines().next().unwrap_or("").to_owned(),
            Output::Facts(f) => format!("{} facts", f.len()),
            Output::Table(t) => format!("{} rows", t.rows.len()),
        }
    }
}
