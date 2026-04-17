use clap::ValueEnum;

#[derive(ValueEnum, Clone, Copy, Default)]
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
            Lang::Auto => query.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [T-SL001] Lang::Ja appends Japanese response instruction to query
    #[test]
    fn ja_appends_japanese_instruction() {
        assert_eq!(Lang::Ja.apply_to_query("test"), "test (日本語で回答)");
    }

    /// [T-SL002] Lang::En appends English response instruction to query
    #[test]
    fn en_appends_english_instruction() {
        assert_eq!(Lang::En.apply_to_query("test"), "test (answer in English)");
    }

    /// [T-SL003] Lang::Auto passes query through unchanged
    #[test]
    fn auto_is_passthrough() {
        assert_eq!(Lang::Auto.apply_to_query("test"), "test");
    }
}
