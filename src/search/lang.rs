use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema, Clone, Copy, Default)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Ja,
    En,
    #[default]
    Auto,
}

impl Lang {
    pub fn apply_to_query(self, query: &str) -> String {
        match self {
            Lang::Ja => format!("{query} (日本語で回答)"),
            Lang::En => format!("{query} (answer in English)"),
            Lang::Auto => query.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ja_appends_japanese_instruction() {
        assert_eq!(Lang::Ja.apply_to_query("test"), "test (日本語で回答)");
    }

    #[test]
    fn en_appends_english_instruction() {
        assert_eq!(Lang::En.apply_to_query("test"), "test (answer in English)");
    }

    #[test]
    fn auto_is_passthrough() {
        assert_eq!(Lang::Auto.apply_to_query("test"), "test");
    }
}
