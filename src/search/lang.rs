use clap::ValueEnum;

#[derive(ValueEnum, Clone, Copy, Default)]
pub enum Lang {
    Ja,
    En,
    #[default]
    Auto,
}

impl Lang {
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
