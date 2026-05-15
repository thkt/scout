use clap::ValueEnum;

#[derive(ValueEnum, Clone, Copy, Default)]
pub enum Lang {
    Ja,
    En,
    #[default]
    Auto,
}

impl Lang {
    /// Legacy: appends a Gemini-era response-language instruction to the query string.
    /// Used by `engine::research` (Phase 3 で削除予定)。Brave 経路 (`tools::search`) は
    /// `to_brave_param` を使い、query 文字列を変更しない。
    pub fn apply_to_query(self, query: &str) -> String {
        match self {
            Lang::Ja => format!("{query} (日本語で回答)"),
            Lang::En => format!("{query} (answer in English)"),
            Lang::Auto => query.to_owned(),
        }
    }

    /// Maps `Lang` to the Brave Web Search API's `search_lang` query parameter
    /// (ISO 639-1 code). `Auto` returns `None` so the request omits the parameter
    /// and lets Brave detect the language from the query / IP heuristics.
    pub fn to_brave_param(self) -> Option<&'static str> {
        match self {
            Lang::Ja => Some("ja"),
            Lang::En => Some("en"),
            Lang::Auto => None,
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

    /// [T-SL004] Lang::Ja maps to search_lang=ja
    #[test]
    fn ja_maps_to_brave_ja() {
        assert_eq!(Lang::Ja.to_brave_param(), Some("ja"));
    }

    /// [T-SL005] Lang::En maps to search_lang=en
    #[test]
    fn en_maps_to_brave_en() {
        assert_eq!(Lang::En.to_brave_param(), Some("en"));
    }

    /// [T-SL006] Lang::Auto maps to None (no search_lang parameter)
    #[test]
    fn auto_maps_to_none() {
        assert_eq!(Lang::Auto.to_brave_param(), None);
    }
}
