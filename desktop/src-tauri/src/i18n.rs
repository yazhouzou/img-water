//! 极简双语文案：界面传入 `lang`（zh/en），缺省 zh，保证 CLI/旧调用行为不变。

/// 按语言二选一。`lang` 以 "zh" 开头取中文，否则取英文。
pub fn tr(lang: &str, zh: &str, en: &str) -> String {
    if lang.to_lowercase().starts_with("zh") {
        zh.to_string()
    } else {
        en.to_string()
    }
}

/// 归一化可选语言参数，缺省 zh。
pub fn load(lang: Option<String>) -> String {
    lang.unwrap_or_else(|| "zh".to_string())
}
