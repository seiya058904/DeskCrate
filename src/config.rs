//! Portable `config.toml`: typed view + format-preserving writes (toml_edit keeps
//! the user's comments and layout). Unknown keys are ignored, bad values fall back
//! to defaults, so a typo never prevents start-up.

use std::path::{Path, PathBuf};

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

use crate::util::fnv1a;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Desktop,
    Drawer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemePref {
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Material {
    Mica,
    MicaAlt,
    Acrylic,
    Blur,
    Solid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    Square,
    Small,
    Round,
    Large,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollapseMode {
    Expanded,
    Click,
    Smart,
}

#[derive(Clone, Debug, PartialEq)]
pub struct General {
    pub mode: Mode,
    pub theme: ThemePref,
    pub hotkey: String,
    pub autostart: bool,
    pub log: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Appearance {
    pub material: Material,
    pub tint: f32,
    pub corner: Corner,
    pub icon_size: f32,
    pub text_size: f32,
    pub label_lines: u32,
    pub text_shadow: bool,
    pub show_hidden: bool,
    pub spacing_h: f32,
    pub spacing_v: f32,
    pub name_width: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CapsuleCfg {
    pub expand_delay_ms: u32,
    pub collapse_delay_ms: u32,
    pub animation_ms: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    None,
    Left,
    Right,
    Top,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DrawerCfg {
    pub edge: Edge,
    pub width: f32,
    /// Fraction of the edge (start, end) that reacts to the pointer.
    pub hot_zone: (f32, f32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct GridCfg {
    pub id: String,
    pub title: String,
    pub path: String,
    pub monitor: String,
    /// DIP, relative to the monitor work area. `None` = auto-place.
    pub rect: Option<[f32; 4]>,
    pub collapse: CollapseMode,
    pub collapsed: bool,
    pub locked: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub general: General,
    pub appearance: Appearance,
    pub capsule: CapsuleCfg,
    pub drawer: DrawerCfg,
    pub sort: crate::sort::SortCfg,
    pub grids: Vec<GridCfg>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            general: General {
                mode: Mode::Desktop,
                theme: ThemePref::System,
                hotkey: "Ctrl+Alt+D".into(),
                autostart: false,
                log: false,
            },
            appearance: Appearance {
                material: Material::Mica,
                tint: 0.18,
                corner: Corner::Round,
                icon_size: 30.0,
                text_size: 11.5,
                label_lines: 2,
                text_shadow: true,
                show_hidden: false,
                spacing_h: 0.40,
                spacing_v: 0.60,
                name_width: 0.36,
            },
            capsule: CapsuleCfg { expand_delay_ms: 360, collapse_delay_ms: 620, animation_ms: 220 },
            drawer: DrawerCfg { edge: Edge::Right, width: 380.0, hot_zone: (0.25, 0.75) },
            sort: crate::sort::SortCfg::default(),
            grids: Vec::new(),
        }
    }
}

// ---- typed getters ---------------------------------------------------------

fn get<'a>(doc: &'a Item, table: &str, key: &str) -> Option<&'a Item> {
    doc.get(table)?.get(key)
}

fn as_f32(item: Option<&Item>) -> Option<f32> {
    let v = item?;
    v.as_float().map(|f| f as f32).or_else(|| v.as_integer().map(|i| i as f32))
}

fn f32_in(item: Option<&Item>, default: f32, lo: f32, hi: f32) -> f32 {
    as_f32(item).filter(|v| v.is_finite()).map(|v| v.clamp(lo, hi)).unwrap_or(default)
}

fn u32_in(item: Option<&Item>, default: u32, lo: u32, hi: u32) -> u32 {
    item.and_then(|v| v.as_integer()).map(|v| (v.max(0) as u32).clamp(lo, hi)).unwrap_or(default)
}

fn bool_or(item: Option<&Item>, default: bool) -> bool {
    item.and_then(|v| v.as_bool()).unwrap_or(default)
}

fn str_or(item: Option<&Item>, default: &str) -> String {
    item.and_then(|v| v.as_str()).map(str::to_owned).unwrap_or_else(|| default.to_owned())
}

fn str_list(item: Option<&Item>) -> Vec<String> {
    item.and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

fn parse_sort(root: &Item) -> crate::sort::SortCfg {
    let d = crate::sort::SortCfg::default();
    let mut rules = Vec::new();
    if let Some(aot) = root.get("sort").and_then(|t| t.get("rule")).and_then(Item::as_array_of_tables) {
        for t in aot.iter() {
            let rule = crate::sort::Rule {
                name: str_or(t.get("name"), "规则"),
                extensions: str_list(t.get("extensions"))
                    .into_iter()
                    .map(|e| e.trim_start_matches('.').to_lowercase())
                    .collect(),
                keywords: str_list(t.get("keywords")),
                idle_days: u32_in(t.get("idle_days"), 0, 0, 3650),
                target: str_or(t.get("target"), ""),
            };
            if rule.is_meaningful() {
                rules.push(rule);
            }
        }
    }
    crate::sort::SortCfg {
        enabled: bool_or(get(root, "sort", "enabled"), d.enabled),
        dry_run: bool_or(get(root, "sort", "dry_run"), d.dry_run),
        watch: str_list(get(root, "sort", "watch")),
        settle_seconds: u32_in(get(root, "sort", "settle_seconds"), d.settle_seconds, 2, 3600),
        include_folders: bool_or(get(root, "sort", "include_folders"), d.include_folders),
        rules,
    }
}

fn enum_or<T: Copy>(item: Option<&Item>, default: T, map: &[(&str, T)]) -> T {
    let Some(s) = item.and_then(|v| v.as_str()) else { return default };
    let s = s.trim().to_ascii_lowercase();
    map.iter().find(|(k, _)| *k == s).map(|(_, v)| *v).unwrap_or(default)
}

pub fn parse(doc: &DocumentMut) -> Config {
    let root = doc.as_item();
    let d = Config::default();
    let general = General {
        mode: enum_or(
            get(root, "general", "mode"),
            d.general.mode,
            &[("desktop", Mode::Desktop), ("drawer", Mode::Drawer)],
        ),
        theme: enum_or(
            get(root, "general", "theme"),
            d.general.theme,
            &[("system", ThemePref::System), ("dark", ThemePref::Dark), ("light", ThemePref::Light)],
        ),
        hotkey: str_or(get(root, "general", "hotkey"), &d.general.hotkey),
        autostart: bool_or(get(root, "general", "autostart"), false),
        log: bool_or(get(root, "general", "log"), false),
    };
    let a = &d.appearance;
    let appearance = Appearance {
        material: enum_or(
            get(root, "appearance", "material"),
            a.material,
            &[
                ("mica", Material::Mica),
                ("mica_alt", Material::MicaAlt),
                ("acrylic", Material::Acrylic),
                ("blur", Material::Blur),
                ("solid", Material::Solid),
            ],
        ),
        tint: f32_in(get(root, "appearance", "tint"), a.tint, 0.0, 1.0),
        corner: enum_or(
            get(root, "appearance", "corner"),
            a.corner,
            &[("square", Corner::Square), ("small", Corner::Small), ("round", Corner::Round), ("large", Corner::Large)],
        ),
        icon_size: f32_in(get(root, "appearance", "icon_size"), a.icon_size, 24.0, 56.0),
        text_size: f32_in(get(root, "appearance", "text_size"), a.text_size, 9.0, 18.0),
        label_lines: u32_in(get(root, "appearance", "label_lines"), a.label_lines, 0, 3),
        text_shadow: bool_or(get(root, "appearance", "text_shadow"), a.text_shadow),
        show_hidden: bool_or(get(root, "appearance", "show_hidden"), a.show_hidden),
        spacing_h: f32_in(get(root, "appearance", "spacing_h"), a.spacing_h, 0.0, 1.0),
        spacing_v: f32_in(get(root, "appearance", "spacing_v"), a.spacing_v, 0.0, 1.0),
        name_width: f32_in(get(root, "appearance", "name_width"), a.name_width, 0.0, 1.0),
    };
    let c = &d.capsule;
    let capsule = CapsuleCfg {
        expand_delay_ms: u32_in(get(root, "capsule", "expand_delay_ms"), c.expand_delay_ms, 0, 3000),
        collapse_delay_ms: u32_in(get(root, "capsule", "collapse_delay_ms"), c.collapse_delay_ms, 0, 5000),
        animation_ms: u32_in(get(root, "capsule", "animation_ms"), c.animation_ms, 0, 1000),
    };
    let dz = &d.drawer;
    let zone = root
        .get("drawer")
        .and_then(|t| t.get("hot_zone"))
        .and_then(|v| v.as_array())
        .and_then(|a| {
            let f = |i: usize| a.get(i).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|n| n as f64)));
            Some((f(0)? as f32, f(1)? as f32))
        })
        .map(|(a, b)| (a.clamp(0.0, 1.0).min(b.clamp(0.0, 1.0)), b.clamp(0.0, 1.0).max(a.clamp(0.0, 1.0))))
        .filter(|(a, b)| b - a >= 0.05)
        .unwrap_or(dz.hot_zone);
    let drawer = DrawerCfg {
        edge: enum_or(
            get(root, "drawer", "edge"),
            dz.edge,
            &[("none", Edge::None), ("left", Edge::Left), ("right", Edge::Right), ("top", Edge::Top)],
        ),
        width: f32_in(get(root, "drawer", "width"), dz.width, 240.0, 900.0),
        hot_zone: zone,
    };
    let mut grids = Vec::new();
    if let Some(aot) = root.get("grid").and_then(Item::as_array_of_tables) {
        for t in aot.iter() {
            let item = |k: &str| t.get(k);
            let path = str_or(item("path"), "");
            if path.trim().is_empty() {
                continue;
            }
            let rect = match (as_f32(item("x")), as_f32(item("y")), as_f32(item("w")), as_f32(item("h"))) {
                (Some(x), Some(y), Some(w), Some(h)) if [x, y, w, h].iter().all(|v| v.is_finite()) => {
                    Some([x, y, w.max(crate::theme::MIN_W), h.max(crate::theme::MIN_H)])
                }
                _ => None,
            };
            let id = str_or(item("id"), "");
            grids.push(GridCfg {
                id: if id.is_empty() { format!("grid{}", grids.len() + 1) } else { id },
                title: str_or(item("title"), ""),
                path,
                monitor: str_or(item("monitor"), ""),
                rect,
                collapse: enum_or(
                    item("collapse"),
                    CollapseMode::Expanded,
                    &[
                        ("expanded", CollapseMode::Expanded),
                        ("click", CollapseMode::Click),
                        ("smart", CollapseMode::Smart),
                    ],
                ),
                collapsed: bool_or(item("collapsed"), false),
                locked: bool_or(item("locked"), false),
            });
        }
    }
    // De-duplicate ids so window lookups stay unambiguous.
    let mut seen = std::collections::HashSet::new();
    for (i, g) in grids.iter_mut().enumerate() {
        if !seen.insert(g.id.clone()) {
            g.id = format!("{}-{}", g.id, i + 1);
            seen.insert(g.id.clone());
        }
    }
    let sort = parse_sort(root);
    Config { general, appearance, capsule, drawer, sort, grids }
}

/// Expands `~`, `%VAR%` and known-folder tokens (`{desktop}`, `{downloads}`, `{documents}`).
pub fn expand_path(raw: &str) -> String {
    let s = raw.trim();
    let lower = s.to_ascii_lowercase();
    for (token, folder) in [
        ("{desktop}", crate::shell::KnownFolder::Desktop),
        ("{downloads}", crate::shell::KnownFolder::Downloads),
        ("{documents}", crate::shell::KnownFolder::Documents),
    ] {
        if lower.starts_with(token)
            && let Some(base) = crate::shell::known_folder(folder)
        {
            return format!("{}{}", base, &s[token.len()..]).replace('/', "\\");
        }
    }
    let s = if let Some(rest) = s.strip_prefix('~') {
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        format!("{home}{rest}")
    } else {
        s.to_owned()
    };
    crate::shell::expand_env(&s).replace('/', "\\")
}

pub struct ConfigStore {
    pub path: PathBuf,
    pub doc: DocumentMut,
    pub cfg: Config,
    last_hash: u64,
}

impl ConfigStore {
    /// Portable location: next to the executable when writable, otherwise
    /// `%LOCALAPPDATA%\DeskCrate`.
    pub fn locate() -> PathBuf {
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
        if let Some(dir) = exe_dir
            && dir_writable(&dir)
        {
            return dir.join("config.toml");
        }
        let base = std::env::var("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
        let dir = base.join("DeskCrate");
        let _ = std::fs::create_dir_all(&dir);
        dir.join("config.toml")
    }

    pub fn load_or_create(path: PathBuf) -> Self {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => {
                let t = default_template();
                if let Err(e) = write_atomic(&path, t.as_bytes()) {
                    crate::log!("config: cannot write default ({e})");
                }
                t
            }
        };
        let doc = text.parse::<DocumentMut>().unwrap_or_else(|e| {
            crate::log!("config: parse error, using defaults: {e}");
            DocumentMut::new()
        });
        let cfg = parse(&doc);
        let mut store = ConfigStore { path, doc, cfg, last_hash: fnv1a(text.as_bytes()) };
        store.ensure_ids();
        store
    }

    /// Re-reads the file. Returns `Ok(true)` when the content changed and parsed.
    pub fn reload_if_changed(&mut self) -> Result<bool, String> {
        let text = std::fs::read_to_string(&self.path).map_err(|e| e.to_string())?;
        let h = fnv1a(text.as_bytes());
        if h == self.last_hash {
            return Ok(false);
        }
        self.last_hash = h;
        let doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
        self.cfg = parse(&doc);
        self.doc = doc;
        self.ensure_ids();
        Ok(true)
    }

    pub fn save(&mut self) {
        let text = self.doc.to_string();
        self.last_hash = fnv1a(text.as_bytes());
        if let Err(e) = write_atomic(&self.path, text.as_bytes()) {
            crate::log!("config: save failed: {e}");
        }
        self.cfg = parse(&self.doc);
    }

    fn grid_table_mut(&mut self, id: &str) -> Option<&mut Table> {
        self.doc
            .get_mut("grid")
            .and_then(Item::as_array_of_tables_mut)?
            .iter_mut()
            .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(id))
    }

    /// Writes missing grid ids back so positions can be persisted per grid.
    fn ensure_ids(&mut self) {
        let mut changed = false;
        if let Some(aot) = self.doc.get_mut("grid").and_then(Item::as_array_of_tables_mut) {
            for (i, t) in aot.iter_mut().enumerate() {
                if t.get("id").and_then(|v| v.as_str()).is_none_or(str::is_empty)
                    && let Some(g) = self.cfg.grids.get(i)
                {
                    t.insert("id", value(g.id.clone()));
                    changed = true;
                }
            }
        }
        if changed {
            self.save();
        }
    }

    pub fn set_grid_rect(&mut self, id: &str, monitor: &str, r: [f32; 4]) {
        let round = |v: f32| ((v * 10.0).round() / 10.0) as f64;
        if let Some(t) = self.grid_table_mut(id) {
            t.insert("monitor", value(monitor));
            t.insert("x", value(round(r[0])));
            t.insert("y", value(round(r[1])));
            t.insert("w", value(round(r[2])));
            t.insert("h", value(round(r[3])));
        }
        self.save();
    }

    pub fn set_grid_flag(&mut self, id: &str, key: &str, v: bool) {
        if let Some(t) = self.grid_table_mut(id) {
            t.insert(key, value(v));
        }
        self.save();
    }

    pub fn set_grid_str(&mut self, id: &str, key: &str, v: &str) {
        if let Some(t) = self.grid_table_mut(id) {
            t.insert(key, value(v));
        }
        self.save();
    }

    pub fn set_value(&mut self, table: &str, key: &str, v: toml_edit::Value) {
        if self.doc.get(table).and_then(Item::as_table).is_none() {
            self.doc.insert(table, Item::Table(Table::new()));
        }
        if let Some(t) = self.doc.get_mut(table).and_then(Item::as_table_mut) {
            t.insert(key, Item::Value(v));
        }
        self.save();
    }

    pub fn add_grid(&mut self, title: &str, path: &str) -> String {
        let id = format!("g{:x}", crate::util::tick_ms());
        let mut t = Table::new();
        t.insert("id", value(id.clone()));
        t.insert("title", value(title));
        t.insert("path", value(path));
        t.insert("collapse", value("expanded"));
        if self.doc.get("grid").and_then(Item::as_array_of_tables).is_none() {
            self.doc.insert("grid", Item::ArrayOfTables(ArrayOfTables::new()));
        }
        if let Some(aot) = self.doc.get_mut("grid").and_then(Item::as_array_of_tables_mut) {
            aot.push(t);
        }
        self.save();
        id
    }

    pub fn remove_grid(&mut self, id: &str) {
        if let Some(aot) = self.doc.get_mut("grid").and_then(Item::as_array_of_tables_mut) {
            aot.retain(|t| t.get("id").and_then(|v| v.as_str()) != Some(id));
        }
        self.save();
    }
}

fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(".deskcrate-write-probe");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Write to a temp file, then replace — a crash never leaves a half-written config.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

fn toml_literal(s: &str) -> String {
    if s.contains('\'') || s.contains('\n') { format!("{:?}", s) } else { format!("'{s}'") }
}

pub fn default_template() -> String {
    let desktop = crate::shell::known_folder(crate::shell::KnownFolder::Desktop).unwrap_or_else(|| "~\\Desktop".into());
    let downloads =
        crate::shell::known_folder(crate::shell::KnownFolder::Downloads).unwrap_or_else(|| "~\\Downloads".into());
    format!(
        r#"# DeskCrate 配置文件（保存后自动热重载）
# 每个格子都只是真实文件夹的“视图”：删除本程序或本文件，不会影响任何文件。

[general]
mode = "desktop"        # desktop = 桌面收纳格；drawer = 空桌面 + 边缘/热键呼出抽屉
theme = "system"        # system | dark | light
hotkey = "Ctrl+Alt+D"   # 抽屉模式下呼出抽屉；桌面模式下临时把全部格子提到最前
autostart = false       # 开机自启（唯一会写注册表的选项，关闭即删除）
log = false             # 写 deskcrate.log 便于排障

[appearance]
material = "mica"       # mica | mica_alt | acrylic | blur | solid
tint = 0.18             # 材质上方着色层的不透明度 0~1（solid/blur 时为背景不透明度）
corner = "round"        # square | small | round | large
icon_size = 30          # 图标尺寸 DIP（24~56）
text_size = 11.5        # 文件名字号
label_lines = 2         # 文件名最多行数 0~3（0 = 隐藏文件名）
text_shadow = true      # 文件名阴影，浅色壁纸上更易读
show_hidden = false     # 显示隐藏文件
spacing_h = 0.40        # 水平间距 0~1
spacing_v = 0.60        # 垂直间距 0~1
name_width = 0.36       # 文件名宽度 0~1

[capsule]
expand_delay_ms = 360   # smart 模式：悬停多久展开
collapse_delay_ms = 620 # smart 模式：移出多久收起
animation_ms = 220      # 展开/收起动画时长（0 = 无动画）

[drawer]                # mode = "drawer" 时生效：桌面保持 100% 干净，按需呼出聚合抽屉
edge = "right"          # 呼出边缘：left | right | top | none（none = 仅热键）
width = 380             # 抽屉宽度 DIP
hot_zone = [0.25, 0.75] # 边缘上响应指针的区段（避开屏幕角落）

# ---- 极简自动归类：只移动、从不删除；重名自动加 (2) 后缀；跳过下载中/锁定的文件 ----
# 托盘「立即整理…」随时可用：先预览、确认后才移动；每批都可「撤销上次归档」。
[sort]
enabled = false         # true = 监听下列目录，文件静止 settle_seconds 秒后自动按规则归档
dry_run = true          # enabled 时仅预览（托盘气泡提示），不实际移动
watch = [{desktop_w}, {downloads_w}]
settle_seconds = 10
include_folders = false

# 规则按顺序匹配，给出的条件需同时满足：extensions（扩展名）/ keywords（文件名关键词）/ idle_days（闲置天数）
[[sort.rule]]
name = "图片"
extensions = ["jpg", "jpeg", "png", "gif", "webp", "bmp", "heic"]
target = '{{documents}}\DeskCrate 归档\图片'

[[sort.rule]]
name = "安装包"
extensions = ["exe", "msi"]
idle_days = 7
target = '{{documents}}\DeskCrate 归档\安装包'

[[sort.rule]]
name = "压缩包"
extensions = ["zip", "rar", "7z"]
idle_days = 7
target = '{{documents}}\DeskCrate 归档\压缩包'

# 每个 [[grid]] 是一个映射到真实目录的收纳格；位置字段（monitor/x/y/w/h）由程序维护。
# collapse: expanded = 常展开 | click = 点标题折叠 | smart = 悬停展开、移出自动收起
[[grid]]
id = "desktop"
title = "桌面"
path = {desktop}
collapse = "expanded"

[[grid]]
id = "downloads"
title = "下载"
path = {downloads}
collapse = "smart"
"#,
        desktop_w = toml_literal(&desktop),
        downloads_w = toml_literal(&downloads),
        desktop = toml_literal(&desktop),
        downloads = toml_literal(&downloads),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_values_fall_back_to_defaults() {
        let doc: DocumentMut = r#"
[appearance]
icon_size = 999
material = "nope"
label_lines = -3
[[grid]]
path = 'C:\x'
x = 1
y = 2
w = 3
h = 4
[[grid]]
path = ""
"#
        .parse()
        .unwrap();
        let c = parse(&doc);
        assert_eq!(c.appearance.icon_size, 56.0);
        assert_eq!(c.appearance.material, Material::Mica);
        assert_eq!(c.appearance.label_lines, 0);
        assert_eq!(c.grids.len(), 1);
        assert_eq!(c.grids[0].rect, Some([1.0, 2.0, crate::theme::MIN_W, crate::theme::MIN_H]));
    }

    #[test]
    fn duplicate_ids_are_made_unique() {
        let doc: DocumentMut = "[[grid]]\nid='a'\npath='x'\n[[grid]]\nid='a'\npath='y'\n".parse().unwrap();
        let c = parse(&doc);
        assert_ne!(c.grids[0].id, c.grids[1].id);
    }
}
