//! 桌面端自定义系统菜单：把最常用的动作放进菜单（并给出标准快捷键提示），让 App 更像
//! 一个正常的 macOS/桌面应用。
//!
//! 设计取舍：**菜单项只负责往前端发事件**，动作仍然由 JS 里已有的按钮点击实现——
//! 同一份逻辑不写两遍，避免 Rust 与 JS 各自演化出不一致的行为（例如"处理中禁止开始"
//! 这类守卫只在 JS 一处）。因此菜单项不随运行状态灰显，点下去由 JS 侧的
//! `disabled` 守卫决定是否生效。

use tauri::menu::{AboutMetadata, Menu, MenuBuilder, MenuEvent, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Runtime};

/// 菜单动作转发到前端的统一事件名（payload = 菜单项 id）。
pub const EVENT_MENU_ACTION: &str = "menu-action";

pub const ID_PICK: &str = "menu-pick";
pub const ID_START: &str = "menu-start";
pub const ID_EXPORT_LOG: &str = "menu-export-log";
pub const ID_CLEANUP: &str = "menu-cleanup";
pub const ID_HELP_GUIDE: &str = "menu-help-guide";
pub const ID_HELP_ISSUE: &str = "menu-help-issue";

/// 需要转发给前端的动作（其余为系统预定义项，由系统原生处理）。
const FORWARDED: &[&str] = &[ID_PICK, ID_START, ID_EXPORT_LOG, ID_CLEANUP];

const GUIDE_URL: &str = "https://github.com/yazhouzou/img-water#readme";
const ISSUE_URL: &str = "https://github.com/yazhouzou/img-water/issues/new";

fn t(lang: &str, zh: &str, en: &str) -> String {
    crate::i18n::tr(lang, zh, en)
}

/// 构建菜单。`lang` 只影响文案，可在运行时切换（见 `apply`）。
pub fn build<R: Runtime>(app: &AppHandle<R>, lang: &str) -> tauri::Result<Menu<R>> {
    let name = "Watermark Cleaner";
    // macOS 的应用菜单（关于/服务/隐藏/退出）。Windows/Linux 上这些预定义项会被忽略或
    // 映射为等价项，不影响其它子菜单。
    let app_menu = SubmenuBuilder::new(app, name)
        .about(Some(AboutMetadata {
            name: Some(name.to_string()),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            ..Default::default()
        }))
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let file_menu = SubmenuBuilder::new(app, t(lang, "文件", "File"))
        .item(
            &MenuItemBuilder::with_id(ID_PICK, t(lang, "选择文件夹…", "Open Folder…"))
                .accelerator("CmdOrCtrl+O")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::with_id(ID_START, t(lang, "开始处理", "Start Processing"))
                .accelerator("CmdOrCtrl+Return")
                .build(app)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::with_id(ID_EXPORT_LOG, t(lang, "导出日志…", "Export Log…"))
                .build(app)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::with_id(ID_CLEANUP, t(lang, "清理临时文件", "Clean Temporary Files"))
                .build(app)?,
        )
        .separator()
        .close_window()
        .build()?;

    // 编辑菜单用系统预定义项：复制/粘贴/全选走原生实现，输入框与日志选区都能用。
    let edit_menu = SubmenuBuilder::new(app, t(lang, "编辑", "Edit"))
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    let help_menu = SubmenuBuilder::new(app, t(lang, "帮助", "Help"))
        .item(&MenuItemBuilder::with_id(ID_HELP_GUIDE, t(lang, "使用说明", "Usage Guide")).build(app)?)
        .item(&MenuItemBuilder::with_id(ID_HELP_ISSUE, t(lang, "反馈问题", "Report an Issue")).build(app)?)
        .build()?;

    MenuBuilder::new(app)
        .items(&[&app_menu, &file_menu, &edit_menu, &help_menu])
        .build()
}

/// 安装/重建菜单。语言可运行时切换，故允许重复调用。
///
/// **失败必须显式可见**：`apply` 失败（加速键写错、平台不支持）时菜单会静默不出现，
/// 只靠 `let _ =` 会变成一个"查不出来的怪现象"——故调用处一律打印/上报错误（见 `app.rs`）。
pub fn apply<R: Runtime>(app: &AppHandle<R>, lang: &str) -> tauri::Result<()> {
    app.set_menu(build(app, lang)?)?;
    Ok(())
}

/// 菜单点击 → 前端事件 / 打开外部链接。返回 true 表示本次事件已处理。
pub fn forward<R: Runtime>(app: &AppHandle<R>, event: &MenuEvent) -> bool {
    let id = event.id().as_ref();
    match id {
        ID_HELP_GUIDE => {
            open_url(GUIDE_URL);
            true
        }
        ID_HELP_ISSUE => {
            open_url(ISSUE_URL);
            true
        }
        _ if FORWARDED.contains(&id) => {
            let _ = app.emit(EVENT_MENU_ACTION, id.to_string());
            true
        }
        _ => false,
    }
}

fn open_url(url: &str) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(not(target_os = "windows"))]
    {
        #[cfg(target_os = "macos")]
        let program = "open";
        #[cfg(all(unix, not(target_os = "macos")))]
        let program = "xdg-open";
        let _ = std::process::Command::new(program).arg(url).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 注：无法在这里用 `tauri::test::mock_app()` 真正构建菜单——macOS 上 muda 的
    // `MenuChild` 只能在主线程创建（`can only be created on the main thread`），而
    // cargo test 的每个用例跑在派生线程里。故菜单构建的兜底是**运行时显式报错**
    // （`app.rs` 的 setup/set_menu_lang 都会把错误打到 stderr 与前端日志），
    // 而不是悄悄咽掉。

    #[test]
    fn forwarded_ids_are_distinct_and_cover_actions() {
        assert!(FORWARDED.contains(&ID_PICK));
        assert!(FORWARDED.contains(&ID_START));
        assert!(FORWARDED.contains(&ID_EXPORT_LOG));
        assert!(FORWARDED.contains(&ID_CLEANUP));
        // 帮助项在 Rust 侧直接打开链接，不进转发列表
        assert!(!FORWARDED.contains(&ID_HELP_GUIDE));
        assert!(!FORWARDED.contains(&ID_HELP_ISSUE));
        let mut ids = FORWARDED.to_vec();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), FORWARDED.len(), "菜单 id 不可重复");
    }

    /// 加速键只用受支持修饰符拼写（拼错会在运行时让菜单创建失败）。
    #[test]
    fn accelerator_strings_use_known_modifiers() {
        const MODS: &[&str] = &["CmdOrCtrl", "Cmd", "Command", "Ctrl", "Control", "Alt", "Option", "Shift", "Super"];
        for accel in ["CmdOrCtrl+O", "CmdOrCtrl+Return"] {
            let (mods, key) = accel.rsplit_once('+').expect("加速键需含 '+'");
            assert!(!key.is_empty(), "{} 缺少主键", accel);
            for m in mods.split('+') {
                assert!(MODS.contains(&m), "未知修饰符 {}（在 {}）", m, accel);
            }
        }
    }
}
