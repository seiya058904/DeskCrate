//! UI strings in the 13 locales upstream DeskBox ships (Strings/*.json).
//!
//! Where a concept exists upstream (rename, refresh, locks, expand modes, item
//! count, tray entries…) its translation is reused verbatim; DeskCrate-only
//! strings follow the same terminology. The locale follows the Windows display
//! language unless `[general] language` names one.

use std::cell::Cell;

/// Column order of [`TABLE`].
const LANGS: [&str; 13] = [
    "zh-CN", "zh-TW", "en-US", "ja-JP", "ko-KR", "de-DE", "fr-FR", "es-ES", "pt-BR", "ru-RU", "ar-SA", "hi-IN", "bn-BD",
];
const EN: usize = 2;

thread_local! {
    static LANG: Cell<usize> = Cell::new(detect());
}

fn detect() -> usize {
    let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    let (primary, sub) = (id & 0x3FF, id >> 10);
    match primary {
        0x04 if sub == 0x02 || sub == 0x04 => 0, // zh-CN, zh-SG
        0x04 => 1,                               // zh-TW, zh-HK, zh-MO
        0x11 => 3,
        0x12 => 4,
        0x07 => 5,
        0x0C => 6,
        0x0A => 7,
        0x16 => 8,
        0x19 => 9,
        0x01 => 10,
        0x39 => 11,
        0x45 => 12,
        _ => EN,
    }
}

/// `"auto"` (or anything unknown) follows Windows; otherwise a tag such as `"en-US"`.
pub fn set_language(tag: &str) {
    let tag = tag.trim();
    let i = LANGS.iter().position(|l| l.eq_ignore_ascii_case(tag)).or_else(|| {
        // Accept a bare language ("en", "zh") as its first listed region.
        let base = tag.split(['-', '_']).next().unwrap_or("");
        (!base.is_empty())
            .then(|| LANGS.iter().position(|l| l.starts_with(&format!("{}-", base.to_ascii_lowercase()))))
            .flatten()
    });
    LANG.with(|l| l.set(i.unwrap_or_else(detect)));
}

pub fn tr(key: &str) -> String {
    let lang = LANG.with(Cell::get);
    match TABLE.iter().find(|(k, _)| *k == key) {
        Some((_, v)) => {
            let s = if v[lang].is_empty() { v[EN] } else { v[lang] };
            s.to_owned()
        }
        None => key.to_owned(),
    }
}

/// `{0}`, `{1}`… placeholders (upstream format).
pub fn trf(key: &str, args: &[&str]) -> String {
    let mut s = tr(key);
    for (i, a) in args.iter().enumerate() {
        s = s.replace(&format!("{{{i}}}"), a);
    }
    s
}

pub fn tr_n(key: &str, n: usize) -> String {
    trf(key, &[&n.to_string()])
}

type Row = (&'static str, [&'static str; 13]);

#[rustfmt::skip]
static TABLE: &[Row] = &[
    // ---- grid menu (upstream CreateMoreFlyout) ----
    ("menu.rename", ["重命名", "重新命名", "Rename", "名前の変更", "이름 바꾸기", "Umbenennen", "Renommer", "Cambiar nombre", "Renomear", "Переименовать", "إعادة تسمية", "नाम बदलें", "নাম পরিবর্তন"]),
    ("menu.refresh", ["刷新", "重新整理", "Refresh", "更新", "새로 고침", "Aktualisieren", "Actualiser", "Actualizar", "Atualizar", "Обновить", "تحديث", "रिफ्रेश करें", "রিফ্রেশ"]),
    ("menu.lock_position", ["锁定位置", "鎖定位置", "Lock position", "位置をロック", "위치 잠금", "Position sperren", "Verrouiller la position", "Bloquear posición", "Bloquear posição", "Зафиксировать положение", "قفل الموضع", "स्थिति लॉक करें", "লক অবস্থান"]),
    ("menu.lock_size", ["锁定尺寸", "鎖定尺寸", "Lock size", "サイズをロック", "크기 잠금", "Größe sperren", "Verrouiller la taille", "Bloquear tamaño", "Bloquear tamanho", "Зафиксировать размер", "قفل الحجم", "आकार लॉक करें", "লক সাইজ"]),
    ("menu.delete_grid", ["删除格子", "刪除格子", "Delete grid", "ウィジェットを削除", "위젯 삭제", "Widget löschen", "Supprimer le widget", "Eliminar widget", "Excluir widget", "Удалить виджет", "حذف الأداة", "विजेट हटाएँ", "উইজেট মুছুন"]),
    ("menu.open_in_explorer", ["在资源管理器中打开", "在檔案總管中開啟", "Open in File Explorer", "エクスプローラーで開く", "파일 탐색기에서 열기", "Im Datei-Explorer öffnen", "Ouvrir dans l’Explorateur de fichiers", "Abrir en el Explorador de archivos", "Abrir no Explorador de Arquivos", "Открыть в Проводнике", "فتح في مستكشف الملفات", "फ़ाइल एक्सप्लोरर में खोलें", "ফাইল এক্সপ্লোরারে খুলুন"]),
    ("collapse.expanded", ["始终展开", "始終展開", "Always expanded", "展開を維持", "항상 펼치기", "Ausgeklappt lassen", "Toujours développé", "Mantener ampliado", "Manter expandido", "Всегда развернут", "حافظ على التوسع", "विस्तारित रखें", "প্রসারিত রাখুন"]),
    ("collapse.click", ["单击展开", "按一下展開", "Click to expand", "クリックで切り替え", "클릭하여 펼치기", "Klick zum Ausklappen", "Développement par clic", "Haz clic para expandir", "Clique para expandir", "Развернуть по щелчку", "انقر للتوسيع", "टॉगल करने के लिए क्लिक करें", "টগল করতে ক্লিক করুন"]),
    ("collapse.smart", ["悬停自动展开", "暫留自動展開", "Expand on hover", "スマート展開", "마우스 올리면 펼치기", "Bei Hover ausklappen", "Développement au survol", "Ampliación inteligente", "Expandir ao fazer Hover", "Развернуть при наведении", "التوسيع عند التحويم", "स्मार्ट विस्तार", "স্মার্ট প্রসারিত"]),
    ("capsule.count", ["{0} 项", "{0} 項", "{0} items", "{0} アイテム", "항목 {0}개", "{0} Elemente", "{0} éléments", "{0} elementos", "{0} itens", "{0} элементов", "{0} عنصر (عناصر)", "{0} आइटम", "{0} আইটেম"]),
    ("toast.icon_size", ["图标大小 {0}", "圖示大小 {0}", "Icon size {0}", "アイコンサイズ {0}", "아이콘 크기 {0}", "Symbolgröße {0}", "Taille de l'icône {0}", "Tamaño del icono {0}", "Tamanho do ícone {0}", "Размер значка {0}", "حجم الأيقونة {0}", "चिह्न का आकार {0}", "আইকনের আকার {0}"]),
    // ---- grid content ----
    ("grid.not_found", ["找不到文件夹", "找不到資料夾", "Folder not found", "フォルダーが見つかりません", "폴더를 찾을 수 없음", "Ordner nicht gefunden", "Dossier introuvable", "No se encuentra la carpeta", "Pasta não encontrada", "Папка не найдена", "المجلد غير موجود", "फ़ोल्डर नहीं मिला", "ফোল্ডার পাওয়া যায়নি"]),
    ("grid.read_error", ["无法读取文件夹（错误 {0}）", "無法讀取資料夾（錯誤 {0}）", "Can’t read the folder (error {0})", "フォルダーを読み取れません (エラー {0})", "폴더를 읽을 수 없음(오류 {0})", "Ordner kann nicht gelesen werden (Fehler {0})", "Impossible de lire le dossier (erreur {0})", "No se puede leer la carpeta (error {0})", "Não foi possível ler a pasta (erro {0})", "Не удалось прочитать папку (ошибка {0})", "تعذرت قراءة المجلد (الخطأ {0})", "फ़ोल्डर पढ़ा नहीं जा सका (त्रुटि {0})", "ফোল্ডার পড়া যায়নি (ত্রুটি {0})"]),
    ("grid.no_match", ["没有与“{0}”匹配的项目", "沒有符合「{0}」的項目", "No items match “{0}”", "「{0}」に一致する項目はありません", "“{0}”과(와) 일치하는 항목 없음", "Keine Elemente für „{0}“", "Aucun élément ne correspond à « {0} »", "Ningún elemento coincide con «{0}»", "Nenhum item corresponde a “{0}”", "Нет элементов, соответствующих «{0}»", "لا توجد عناصر تطابق \"{0}\"", "“{0}” से मेल खाता कोई आइटम नहीं", "“{0}”-এর সাথে মেলে এমন কোনো আইটেম নেই"]),
    ("grid.empty", ["空文件夹 — 把文件拖到这里", "空資料夾 — 將檔案拖曳到這裡", "Empty folder — drop files here", "空のフォルダー — ここにファイルをドロップ", "빈 폴더 — 여기에 파일을 끌어 놓으세요", "Leerer Ordner – Dateien hierher ziehen", "Dossier vide — déposez des fichiers ici", "Carpeta vacía: suelta archivos aquí", "Pasta vazia — solte arquivos aqui", "Пустая папка — перетащите файлы сюда", "مجلد فارغ — أفلت الملفات هنا", "खाली फ़ोल्डर — फ़ाइलें यहाँ छोड़ें", "খালি ফোল্ডার — এখানে ফাইল ছাড়ুন"]),
    ("grid.everything", ["Everything · 全盘结果  {0}", "Everything · 全磁碟結果  {0}", "Everything · all drives  {0}", "Everything · 全ドライブ  {0}", "Everything · 전체 드라이브  {0}", "Everything · alle Laufwerke  {0}", "Everything · tous les lecteurs  {0}", "Everything · todas las unidades  {0}", "Everything · todas as unidades  {0}", "Everything · все диски  {0}", "Everything · كل محركات الأقراص  {0}", "Everything · सभी ड्राइव  {0}", "Everything · সব ড্রাইভ  {0}"]),
    // ---- tray (upstream App.Tray) ----
    ("tray.tip_normal", ["DeskCrate 桌面收纳 · 左键单击置顶格子", "DeskCrate 桌面收納 - 按一下滑鼠左鍵置頂格子", "DeskCrate — left-click to raise grids", "DeskCrate - 左クリックでウィジェットを表示", "DeskCrate · 왼쪽 클릭으로 위젯을 맨 위에 표시", "DeskCrate - Linksklick zum Einblenden der Widgets", "DeskCrate – clic gauche pour afficher les widgets", "DeskCrate: clic izquierdo para mostrar los widgets", "DeskCrate - clique com o botão esquerdo para elevar os widgets", "DeskCrate — щелкните левой кнопкой мыши, чтобы поднять виджеты", "DeskCrate — انقر بزر الماوس الأيسر لإظهار الأدوات", "DeskCrate - विजेट सामने लाने के लिए बायाँ-क्लिक करें", "DeskCrate — উইজেট সামনে আনতে বাম-ক্লিক করুন"]),
    ("tray.tip_raised", ["DeskCrate 桌面收纳 · 格子已临时置顶", "DeskCrate 桌面收納 - 格子已臨時置頂", "DeskCrate — grids temporarily raised", "DeskCrate - ウィジェットが一時的に前面表示中", "DeskCrate · 위젯이 일시적으로 맨 위에 표시됨", "DeskCrate - Widgets vorübergehend eingeblendet", "DeskCrate – widgets temporairement au premier plan", "DeskCrate: widgets temporalmente al frente", "DeskCrate - widgets elevados temporariamente", "DeskCrate — виджеты временно подняты", "DeskCrate — الأدوات مرفوعة مؤقتًا", "DeskCrate - विजेट अस्थायी रूप से सामने लाए गए", "DeskCrate — উইজেটগুলি সাময়িকভাবে সামনে তোলা আছে"]),
    ("tray.new_grid", ["新建格子…", "建立格子…", "New grid…", "新しいウィジェット…", "새 위젯…", "Neues Widget…", "Nouveau widget…", "Nuevo widget…", "Novo widget…", "Новый виджет…", "أداة جديدة…", "नया विजेट…", "নতুন উইজেট…"]),
    ("tray.refresh_all", ["刷新全部", "全部重新整理", "Refresh all", "すべて更新", "모두 새로 고침", "Alle aktualisieren", "Tout actualiser", "Actualizar todo", "Atualizar tudo", "Обновить все", "تحديث الكل", "सभी रिफ्रेश करें", "সব রিফ্রেশ"]),
    ("tray.show_all", ["显示全部格子", "顯示全部格子", "Show all grids", "すべてのウィジェットを表示", "모든 위젯 표시", "Alle Widgets anzeigen", "Afficher tous les widgets", "Mostrar todos los widgets", "Mostrar todos os widgets", "Показать все виджеты", "إظهار كل الأدوات", "सभी विजेट दिखाएँ", "সব উইজেট দেখান"]),
    ("tray.hide_all", ["隐藏全部格子", "隱藏全部格子", "Hide all grids", "すべてのウィジェットを非表示", "모든 위젯 숨기기", "Alle Widgets ausblenden", "Masquer tous les widgets", "Ocultar todos los widgets", "Ocultar todos os widgets", "Скрыть все виджеты", "إخفاء كل الأدوات", "सभी विजेट छिपाएँ", "সব উইজেট লুকান"]),
    ("tray.drawer_mode", ["抽屉模式（桌面保持空白）", "抽屜模式（桌面保持空白）", "Drawer mode (keep the desktop empty)", "ドロワーモード（デスクトップを空に保つ）", "서랍 모드(바탕 화면을 비워 둠)", "Schubladenmodus (Desktop bleibt leer)", "Mode tiroir (bureau vide)", "Modo cajón (escritorio vacío)", "Modo gaveta (área de trabalho vazia)", "Режим ящика (рабочий стол пуст)", "وضع الدرج (سطح مكتب فارغ)", "ड्रॉअर मोड (डेस्कटॉप खाली रखें)", "ড্রয়ার মোড (ডেস্কটপ খালি রাখুন)"]),
    ("tray.open_drawer", ["打开抽屉", "開啟抽屜", "Open drawer", "ドロワーを開く", "서랍 열기", "Schublade öffnen", "Ouvrir le tiroir", "Abrir cajón", "Abrir gaveta", "Открыть ящик", "فتح الدرج", "ड्रॉअर खोलें", "ড্রয়ার খুলুন"]),
    ("tray.sort_now", ["立即整理…（先预览）", "立即整理…（先預覽）", "Sort now… (preview first)", "今すぐ整理…（先にプレビュー）", "지금 정리…(먼저 미리 보기)", "Jetzt aufräumen… (mit Vorschau)", "Ranger maintenant… (aperçu d’abord)", "Ordenar ahora… (vista previa)", "Organizar agora… (com prévia)", "Упорядочить сейчас… (с предпросмотром)", "ترتيب الآن… (معاينة أولًا)", "अभी व्यवस्थित करें… (पहले पूर्वावलोकन)", "এখনই সাজান… (আগে প্রিভিউ)"]),
    ("tray.sort_undo", ["撤销上次归档（{0} 个文件）", "復原上次歸檔（{0} 個檔案）", "Undo last sort ({0} files)", "前回の整理を元に戻す（{0} 個のファイル）", "마지막 정리 실행 취소(파일 {0}개)", "Letztes Aufräumen rückgängig ({0} Dateien)", "Annuler le dernier rangement ({0} fichiers)", "Deshacer última ordenación ({0} archivos)", "Desfazer última organização ({0} arquivos)", "Отменить последнее упорядочивание ({0} файлов)", "التراجع عن آخر ترتيب ({0} ملفات)", "पिछली व्यवस्था पूर्ववत करें ({0} फ़ाइलें)", "শেষ সাজানো পূর্বাবস্থায় ফেরান ({0}টি ফাইল)"]),
    ("tray.edit_config", ["编辑配置文件", "編輯設定檔", "Edit config file", "設定ファイルを編集", "구성 파일 편집", "Konfigurationsdatei bearbeiten", "Modifier le fichier de configuration", "Editar archivo de configuración", "Editar arquivo de configuração", "Изменить файл настроек", "تحرير ملف الإعدادات", "कॉन्फ़िग फ़ाइल संपादित करें", "কনফিগ ফাইল সম্পাদনা"]),
    ("tray.open_config_dir", ["打开配置所在文件夹", "開啟設定所在資料夾", "Open config folder", "設定フォルダーを開く", "구성 폴더 열기", "Konfigurationsordner öffnen", "Ouvrir le dossier de configuration", "Abrir carpeta de configuración", "Abrir pasta de configuração", "Открыть папку настроек", "فتح مجلد الإعدادات", "कॉन्फ़िग फ़ोल्डर खोलें", "কনফিগ ফোল্ডার খুলুন"]),
    ("tray.autostart", ["开机启动", "開機啟動", "Launch at startup", "スタートアップ時に起動", "시작 시 자동 실행", "Mit Windows starten", "Lancer au démarrage", "Iniciar con Windows", "Iniciar com o sistema", "Запускать вместе с Windows", "التشغيل مع بدء النظام", "स्टार्टअप पर चलाएँ", "স্টার্টআপে চালু করুন"]),
    ("tray.about", ["关于 DeskCrate", "關於 DeskCrate", "About DeskCrate", "DeskCrate について", "DeskCrate 정보", "Über DeskCrate", "À propos de DeskCrate", "Acerca de DeskCrate", "Sobre o DeskCrate", "О DeskCrate", "حول DeskCrate", "DeskCrate के बारे में", "DeskCrate সম্পর্কে"]),
    ("tray.exit", ["退出", "結束", "Exit", "終了", "종료", "Beenden", "Quitter", "Salir", "Sair", "Выйти", "خروج", "बाहर निकलें", "প্রস্থান"]),
    // ---- dialogs / balloons ----
    ("about.body", ["DeskCrate {0}\n轻量、原生的 Windows 桌面文件收纳工具。\n\n基于 DeskBox（github.com/Tianyu199509/DeskBox，GPL-3.0-only）的非官方 Rust 重写，与原作者无关联，也未获其背书。\n许可证：GPL-3.0-only\n\n配置文件：{1}",
        "DeskCrate {0}\n輕量、原生的 Windows 桌面檔案收納工具。\n\n以 DeskBox（github.com/Tianyu199509/DeskBox，GPL-3.0-only）為基礎的非官方 Rust 重寫，與原作者無關，也未獲其背書。\n授權：GPL-3.0-only\n\n設定檔：{1}",
        "DeskCrate {0}\nA lean, native desktop organizer for Windows.\n\nAn unofficial Rust re-implementation of DeskBox (github.com/Tianyu199509/DeskBox, GPL-3.0-only); not affiliated with or endorsed by its author.\nLicense: GPL-3.0-only\n\nConfig file: {1}",
        "DeskCrate {0}\n軽量でネイティブな Windows デスクトップ整理ツール。\n\nDeskBox（github.com/Tianyu199509/DeskBox、GPL-3.0-only）の非公式な Rust 再実装です。原作者とは無関係で、承認も受けていません。\nライセンス：GPL-3.0-only\n\n設定ファイル：{1}",
        "DeskCrate {0}\n가볍고 네이티브한 Windows 바탕 화면 정리 도구.\n\nDeskBox(github.com/Tianyu199509/DeskBox, GPL-3.0-only)의 비공식 Rust 재구현이며 원작자와 관련이 없고 보증을 받지 않았습니다.\n라이선스: GPL-3.0-only\n\n구성 파일: {1}",
        "DeskCrate {0}\nSchlanke, native Desktop-Organisation für Windows.\n\nInoffizielle Rust-Neuimplementierung von DeskBox (github.com/Tianyu199509/DeskBox, GPL-3.0-only); nicht mit dem Autor verbunden oder von ihm unterstützt.\nLizenz: GPL-3.0-only\n\nKonfigurationsdatei: {1}",
        "DeskCrate {0}\nOrganiseur de bureau natif et léger pour Windows.\n\nRéimplémentation Rust non officielle de DeskBox (github.com/Tianyu199509/DeskBox, GPL-3.0-only), sans lien avec son auteur ni approuvée par lui.\nLicence : GPL-3.0-only\n\nFichier de configuration : {1}",
        "DeskCrate {0}\nOrganizador de escritorio ligero y nativo para Windows.\n\nReimplementación no oficial en Rust de DeskBox (github.com/Tianyu199509/DeskBox, GPL-3.0-only); sin relación con su autor ni respaldada por él.\nLicencia: GPL-3.0-only\n\nArchivo de configuración: {1}",
        "DeskCrate {0}\nOrganizador de área de trabalho leve e nativo para Windows.\n\nReimplementação não oficial em Rust do DeskBox (github.com/Tianyu199509/DeskBox, GPL-3.0-only); sem vínculo nem endosso do autor.\nLicença: GPL-3.0-only\n\nArquivo de configuração: {1}",
        "DeskCrate {0}\nЛёгкий нативный органайзер рабочего стола для Windows.\n\nНеофициальная реализация DeskBox на Rust (github.com/Tianyu199509/DeskBox, GPL-3.0-only); не связана с автором и не одобрена им.\nЛицензия: GPL-3.0-only\n\nФайл настроек: {1}",
        "DeskCrate {0}\nأداة خفيفة وأصلية لتنظيم سطح المكتب في Windows.\n\nإعادة تنفيذ غير رسمية بلغة Rust لـ DeskBox (github.com/Tianyu199509/DeskBox، GPL-3.0-only)؛ لا ترتبط بمؤلفها ولا يعتمدها.\nالترخيص: GPL-3.0-only\n\nملف الإعدادات: {1}",
        "DeskCrate {0}\nWindows के लिए हल्का, नेटिव डेस्कटॉप ऑर्गनाइज़र।\n\nDeskBox (github.com/Tianyu199509/DeskBox, GPL-3.0-only) का अनौपचारिक Rust पुनर्लेखन; मूल लेखक से संबद्ध या समर्थित नहीं।\nलाइसेंस: GPL-3.0-only\n\nकॉन्फ़िग फ़ाइल: {1}",
        "DeskCrate {0}\nWindows-এর জন্য হালকা, নেটিভ ডেস্কটপ অর্গানাইজার।\n\nDeskBox (github.com/Tianyu199509/DeskBox, GPL-3.0-only)-এর অনানুষ্ঠানিক Rust পুনর্লিখন; মূল লেখকের সাথে সম্পর্কিত বা অনুমোদিত নয়।\nলাইসেন্স: GPL-3.0-only\n\nকনফিগ ফাইল: {1}"]),
    ("remove.confirm", ["删除格子「{0}」？\n\n只删除桌面上的这个格子，文件夹「{1}」及其中的文件保持不变。", "刪除格子「{0}」？\n\n只會刪除桌面上的這個格子，資料夾「{1}」及其中的檔案保持不變。", "Delete the grid “{0}”?\n\nOnly the grid is removed from the desktop; the folder “{1}” and its files stay untouched.", "ウィジェット「{0}」を削除しますか？\n\nデスクトップからウィジェットだけを削除します。フォルダー「{1}」とその中のファイルはそのままです。", "위젯 “{0}”을(를) 삭제할까요?\n\n바탕 화면에서 위젯만 제거되며 폴더 “{1}”과 그 안의 파일은 그대로 유지됩니다.", "Widget „{0}“ löschen?\n\nNur das Widget wird vom Desktop entfernt; der Ordner „{1}“ und seine Dateien bleiben erhalten.", "Supprimer le widget « {0} » ?\n\nSeul le widget est retiré du bureau ; le dossier « {1} » et ses fichiers restent intacts.", "¿Eliminar el widget «{0}»?\n\nSolo se quita el widget del escritorio; la carpeta «{1}» y sus archivos no se modifican.", "Excluir o widget “{0}”?\n\nApenas o widget sai da área de trabalho; a pasta “{1}” e seus arquivos permanecem intactos.", "Удалить виджет «{0}»?\n\nС рабочего стола удаляется только виджет; папка «{1}» и её файлы не изменятся.", "حذف الأداة \"{0}\"؟\n\nتتم إزالة الأداة من سطح المكتب فقط؛ يبقى المجلد \"{1}\" وملفاته دون تغيير.", "विजेट “{0}” हटाएँ?\n\nडेस्कटॉप से केवल विजेट हटेगा; फ़ोल्डर “{1}” और उसकी फ़ाइलें वैसी ही रहेंगी।", "উইজেট “{0}” মুছবেন?\n\nশুধু ডেস্কটপ থেকে উইজেটটি সরানো হবে; ফোল্ডার “{1}” ও এর ফাইলগুলো অপরিবর্তিত থাকবে।"]),
    ("pick_folder", ["选择要映射为格子的文件夹", "選擇要對應為格子的資料夾", "Choose a folder to show as a grid", "ウィジェットに表示するフォルダーを選択", "위젯으로 표시할 폴더 선택", "Ordner für das Widget auswählen", "Choisissez un dossier à afficher dans un widget", "Elige una carpeta para el widget", "Escolha uma pasta para o widget", "Выберите папку для виджета", "اختر مجلدًا لعرضه كأداة", "विजेट के रूप में दिखाने के लिए फ़ोल्डर चुनें", "উইজেট হিসেবে দেখাতে একটি ফোল্ডার বেছে নিন"]),
    ("running", ["DeskCrate 已在运行：左键单击托盘图标置顶格子，右键管理。", "DeskCrate 已在執行：按一下通知區域圖示置頂格子，按右鍵管理。", "DeskCrate is already running: left-click the tray icon to raise grids, right-click to manage.", "DeskCrate は実行中です。トレイアイコンを左クリックで前面表示、右クリックで管理。", "DeskCrate가 이미 실행 중입니다. 트레이 아이콘을 왼쪽 클릭하면 맨 위로, 오른쪽 클릭하면 관리합니다.", "DeskCrate läuft bereits: Linksklick auf das Tray-Symbol hebt die Widgets an, Rechtsklick verwaltet sie.", "DeskCrate est déjà lancé : clic gauche sur l’icône pour afficher les widgets, clic droit pour gérer.", "DeskCrate ya se está ejecutando: clic izquierdo en el icono para mostrar los widgets y derecho para gestionarlos.", "O DeskCrate já está em execução: clique com o botão esquerdo no ícone para elevar os widgets e com o direito para gerenciar.", "DeskCrate уже запущен: левый щелчок по значку поднимает виджеты, правый — управление.", "DeskCrate قيد التشغيل بالفعل: انقر بزر الماوس الأيسر على الأيقونة لإظهار الأدوات، وبالأيمن للإدارة.", "DeskCrate पहले से चल रहा है: विजेट सामने लाने के लिए ट्रे आइकन पर बायाँ-क्लिक, प्रबंधन के लिए दायाँ-क्लिक करें।", "DeskCrate ইতিমধ্যে চলছে: উইজেট সামনে আনতে ট্রে আইকনে বাম-ক্লিক, পরিচালনায় ডান-ক্লিক করুন।"]),
    ("hotkey.title", ["DeskCrate 热键不可用", "DeskCrate 快速鍵無法使用", "DeskCrate hotkey unavailable", "DeskCrate のホットキーを使用できません", "DeskCrate 단축키를 사용할 수 없음", "DeskCrate-Tastenkürzel nicht verfügbar", "Raccourci DeskCrate indisponible", "Atajo de DeskCrate no disponible", "Atalho do DeskCrate indisponível", "Сочетание клавиш DeskCrate недоступно", "اختصار DeskCrate غير متاح", "DeskCrate हॉटकी उपलब्ध नहीं", "DeskCrate হটকি অনুপলব্ধ"]),
    ("hotkey.body", ["“{0}” 无效或已被其他程序占用，可在配置文件 [general] hotkey 中修改。", "「{0}」無效或已被其他程式使用，可在設定檔 [general] hotkey 中修改。", "“{0}” is invalid or taken by another app; change it under [general] hotkey in the config file.", "「{0}」は無効か、他のアプリが使用中です。設定ファイルの [general] hotkey で変更できます。", "“{0}”이(가) 잘못되었거나 다른 앱이 사용 중입니다. 구성 파일의 [general] hotkey에서 변경하세요.", "„{0}“ ist ungültig oder belegt; in der Konfigurationsdatei unter [general] hotkey ändern.", "« {0} » est invalide ou déjà utilisé ; modifiez [general] hotkey dans le fichier de configuration.", "«{0}» no es válido o ya está en uso; cámbialo en [general] hotkey del archivo de configuración.", "“{0}” é inválido ou já está em uso; altere em [general] hotkey no arquivo de configuração.", "«{0}» недопустимо или занято; измените [general] hotkey в файле настроек.", "\"{0}\" غير صالح أو مستخدم؛ غيّره في [general] hotkey بملف الإعدادات.", "“{0}” अमान्य है या किसी अन्य ऐप द्वारा उपयोग में है; कॉन्फ़िग फ़ाइल में [general] hotkey बदलें।", "“{0}” অবৈধ বা অন্য অ্যাপ ব্যবহার করছে; কনফিগ ফাইলের [general] hotkey-তে পরিবর্তন করুন।"]),
    ("config.error_title", ["DeskCrate 配置有误", "DeskCrate 設定有誤", "DeskCrate config error", "DeskCrate の設定エラー", "DeskCrate 구성 오류", "DeskCrate-Konfigurationsfehler", "Erreur de configuration DeskCrate", "Error de configuración de DeskCrate", "Erro de configuração do DeskCrate", "Ошибка настроек DeskCrate", "خطأ في إعدادات DeskCrate", "DeskCrate कॉन्फ़िग त्रुटि", "DeskCrate কনফিগ ত্রুটি"]),
    ("config.error_body", ["已保留当前设置：{0}", "已保留目前設定：{0}", "Kept the current settings: {0}", "現在の設定を維持しました：{0}", "현재 설정을 유지했습니다: {0}", "Aktuelle Einstellungen beibehalten: {0}", "Paramètres actuels conservés : {0}", "Se mantuvo la configuración actual: {0}", "Configurações atuais mantidas: {0}", "Текущие настройки сохранены: {0}", "تم الإبقاء على الإعدادات الحالية: {0}", "वर्तमान सेटिंग्स बनी रहीं: {0}", "বর্তমান সেটিংস রাখা হয়েছে: {0}"]),
    // ---- auto-sort ----
    ("sort.preview_title", ["DeskCrate 归档预览", "DeskCrate 歸檔預覽", "DeskCrate sort preview", "DeskCrate 整理プレビュー", "DeskCrate 정리 미리 보기", "DeskCrate-Aufräumvorschau", "Aperçu du rangement DeskCrate", "Vista previa de ordenación de DeskCrate", "Prévia de organização do DeskCrate", "Предпросмотр упорядочивания DeskCrate", "معاينة ترتيب DeskCrate", "DeskCrate व्यवस्था पूर्वावलोकन", "DeskCrate সাজানোর প্রিভিউ"]),
    ("sort.preview_body", ["{0} 个文件符合归档规则（dry_run，未移动）。托盘 →「立即整理…」可查看并执行。", "{0} 個檔案符合歸檔規則（dry_run，未移動）。通知區域 →「立即整理…」可檢視並執行。", "{0} files match the sort rules (dry run, nothing moved). Tray → “Sort now…” to review and run.", "{0} 個のファイルが整理ルールに一致しました（dry_run、未移動）。トレイ →「今すぐ整理…」で確認して実行できます。", "파일 {0}개가 정리 규칙과 일치합니다(dry_run, 이동 안 함). 트레이 → “지금 정리…”에서 확인하고 실행하세요.", "{0} Dateien passen zu den Regeln (Probelauf, nichts verschoben). Tray → „Jetzt aufräumen…“ zum Prüfen und Ausführen.", "{0} fichiers correspondent aux règles (simulation, rien déplacé). Zone de notification → « Ranger maintenant… » pour vérifier et lancer.", "{0} archivos cumplen las reglas (simulación, nada movido). Bandeja → «Ordenar ahora…» para revisar y ejecutar.", "{0} arquivos atendem às regras (simulação, nada movido). Bandeja → “Organizar agora…” para revisar e executar.", "{0} файлов соответствуют правилам (пробный запуск, ничего не перемещено). Трей → «Упорядочить сейчас…» для просмотра и запуска.", "{0} ملفات تطابق القواعد (تشغيل تجريبي، لم يُنقل شيء). الدرج ← \"ترتيب الآن…\" للمراجعة والتنفيذ.", "{0} फ़ाइलें नियमों से मेल खाती हैं (ड्राई रन, कुछ नहीं हटाया)। ट्रे → “अभी व्यवस्थित करें…” से देखें और चलाएँ।", "{0}টি ফাইল নিয়মের সাথে মেলে (ড্রাই রান, কিছু সরানো হয়নি)। ট্রে → “এখনই সাজান…” থেকে দেখুন ও চালান।"]),
    ("sort.auto_title", ["DeskCrate 已自动归档", "DeskCrate 已自動歸檔", "DeskCrate sorted files", "DeskCrate が自動整理しました", "DeskCrate가 자동으로 정리함", "DeskCrate hat aufgeräumt", "DeskCrate a rangé des fichiers", "DeskCrate ordenó archivos", "O DeskCrate organizou arquivos", "DeskCrate упорядочил файлы", "رتّب DeskCrate الملفات", "DeskCrate ने फ़ाइलें व्यवस्थित कीं", "DeskCrate ফাইল সাজিয়েছে"]),
    ("sort.auto_body", ["已移动 {0} 个文件，可在托盘「撤销上次归档」。", "已移動 {0} 個檔案，可在通知區域「復原上次歸檔」。", "Moved {0} files; use “Undo last sort” in the tray to revert.", "{0} 個のファイルを移動しました。トレイの「前回の整理を元に戻す」で戻せます。", "파일 {0}개를 이동했습니다. 트레이의 “마지막 정리 실행 취소”로 되돌릴 수 있습니다.", "{0} Dateien verschoben; „Letztes Aufräumen rückgängig“ im Tray macht es rückgängig.", "{0} fichiers déplacés ; « Annuler le dernier rangement » dans la zone de notification pour revenir.", "Se movieron {0} archivos; usa «Deshacer última ordenación» en la bandeja para revertir.", "{0} arquivos movidos; use “Desfazer última organização” na bandeja para reverter.", "Перемещено файлов: {0}; «Отменить последнее упорядочивание» в трее вернёт их.", "نُقل {0} ملفات؛ استخدم \"التراجع عن آخر ترتيب\" في الدرج للتراجع.", "{0} फ़ाइलें ले जाई गईं; वापस लाने के लिए ट्रे में “पिछली व्यवस्था पूर्ववत करें” चुनें।", "{0}টি ফাইল সরানো হয়েছে; ফেরাতে ট্রের “শেষ সাজানো পূর্বাবস্থায় ফেরান” ব্যবহার করুন।"]),
    ("sort.none", ["没有符合归档规则的文件。", "沒有符合歸檔規則的檔案。", "No files match the sort rules.", "整理ルールに一致するファイルはありません。", "정리 규칙과 일치하는 파일이 없습니다.", "Keine Dateien passen zu den Regeln.", "Aucun fichier ne correspond aux règles.", "Ningún archivo cumple las reglas.", "Nenhum arquivo atende às regras.", "Нет файлов, соответствующих правилам.", "لا توجد ملفات تطابق القواعد.", "कोई फ़ाइल नियमों से मेल नहीं खाती।", "কোনো ফাইল নিয়মের সাথে মেলে না।"]),
    ("sort.confirm_head", ["将按规则移动以下文件（不会删除或覆盖任何文件，可撤销）：", "將依規則移動以下檔案（不會刪除或覆寫任何檔案，可復原）：", "These files will be moved by your rules (nothing is deleted or overwritten; undoable):", "ルールに従って次のファイルを移動します（削除や上書きはせず、元に戻せます）：", "규칙에 따라 다음 파일을 이동합니다(삭제하거나 덮어쓰지 않으며 되돌릴 수 있음):", "Diese Dateien werden nach Ihren Regeln verschoben (nichts wird gelöscht oder überschrieben; rückgängig machbar):", "Ces fichiers seront déplacés selon vos règles (rien n’est supprimé ni écrasé ; annulable) :", "Estos archivos se moverán según tus reglas (no se elimina ni sobrescribe nada; se puede deshacer):", "Estes arquivos serão movidos pelas suas regras (nada é excluído ou substituído; dá para desfazer):", "Эти файлы будут перемещены по правилам (ничего не удаляется и не перезаписывается; можно отменить):", "سيتم نقل هذه الملفات وفق قواعدك (لا يُحذف شيء ولا يُستبدل؛ يمكن التراجع):", "ये फ़ाइलें आपके नियमों के अनुसार ले जाई जाएँगी (कुछ भी हटाया या अधिलेखित नहीं होगा; पूर्ववत संभव):", "এই ফাইলগুলো আপনার নিয়ম অনুযায়ী সরানো হবে (কিছু মোছা বা ওভাররাইট হবে না; ফেরানো যায়):"]),
    ("sort.more", ["…以及另外 {0} 个文件", "…以及另外 {0} 個檔案", "…and {0} more files", "…ほか {0} 個のファイル", "…외 파일 {0}개", "…und {0} weitere Dateien", "…et {0} autres fichiers", "…y {0} archivos más", "…e mais {0} arquivos", "…и ещё {0} файлов", "…و{0} ملفات أخرى", "…और {0} अन्य फ़ाइलें", "…এবং আরও {0}টি ফাইল"]),
    ("sort.confirm_tail", ["共 {0} 个文件，确定执行吗？", "共 {0} 個檔案，確定執行嗎？", "{0} files in total. Go ahead?", "合計 {0} 個のファイル。実行しますか？", "총 파일 {0}개. 실행할까요?", "Insgesamt {0} Dateien. Fortfahren?", "{0} fichiers au total. Continuer ?", "{0} archivos en total. ¿Continuar?", "{0} arquivos no total. Continuar?", "Всего файлов: {0}. Продолжить?", "إجمالي {0} ملفات. هل تريد المتابعة؟", "कुल {0} फ़ाइलें। जारी रखें?", "মোট {0}টি ফাইল। চালিয়ে যাবেন?"]),
    ("sort.confirm_title", ["DeskCrate · 整理预览", "DeskCrate · 整理預覽", "DeskCrate · Sort preview", "DeskCrate · 整理プレビュー", "DeskCrate · 정리 미리 보기", "DeskCrate · Aufräumvorschau", "DeskCrate · Aperçu du rangement", "DeskCrate · Vista previa", "DeskCrate · Prévia", "DeskCrate · Предпросмотр", "DeskCrate · معاينة الترتيب", "DeskCrate · व्यवस्था पूर्वावलोकन", "DeskCrate · সাজানোর প্রিভিউ"]),
    ("sort.moved", ["已移动 {0} 个文件。", "已移動 {0} 個檔案。", "Moved {0} files.", "{0} 個のファイルを移動しました。", "파일 {0}개를 이동했습니다.", "{0} Dateien verschoben.", "{0} fichiers déplacés.", "Se movieron {0} archivos.", "{0} arquivos movidos.", "Перемещено файлов: {0}.", "تم نقل {0} ملفات.", "{0} फ़ाइलें ले जाई गईं।", "{0}টি ফাইল সরানো হয়েছে।"]),
    ("sort.skipped", ["跳过 {0} 个（占用或已变化）。", "略過 {0} 個（使用中或已變更）。", " Skipped {0} (in use or changed).", "{0} 個をスキップしました（使用中または変更済み）。", " {0}개 건너뜀(사용 중이거나 변경됨).", " {0} übersprungen (in Verwendung oder geändert).", " {0} ignorés (en cours d’utilisation ou modifiés).", " Se omitieron {0} (en uso o modificados).", " {0} ignorados (em uso ou alterados).", " Пропущено: {0} (заняты или изменены).", " تم تخطي {0} (قيد الاستخدام أو تغيّرت).", " {0} छोड़ी गईं (उपयोग में या बदली हुई)।", " {0}টি বাদ দেওয়া হয়েছে (ব্যবহৃত বা পরিবর্তিত)।"]),
    ("sort.done_title", ["DeskCrate 整理完成", "DeskCrate 整理完成", "DeskCrate sort finished", "DeskCrate の整理が完了しました", "DeskCrate 정리 완료", "DeskCrate-Aufräumen abgeschlossen", "Rangement DeskCrate terminé", "Ordenación de DeskCrate completada", "Organização do DeskCrate concluída", "Упорядочивание DeskCrate завершено", "اكتمل ترتيب DeskCrate", "DeskCrate व्यवस्था पूरी हुई", "DeskCrate সাজানো সম্পন্ন"]),
    ("sort.undo_moved", ["已移回 {0} 个文件。", "已移回 {0} 個檔案。", "Moved {0} files back.", "{0} 個のファイルを元に戻しました。", "파일 {0}개를 되돌렸습니다.", "{0} Dateien zurückverschoben.", "{0} fichiers remis en place.", "Se devolvieron {0} archivos.", "{0} arquivos devolvidos.", "Возвращено файлов: {0}.", "تمت إعادة {0} ملفات.", "{0} फ़ाइलें वापस लाई गईं।", "{0}টি ফাইল ফেরানো হয়েছে।"]),
    ("sort.undo_skipped", ["{0} 个无法移回（已被移动或删除）。", "{0} 個無法移回（已被移動或刪除）。", " {0} couldn’t be moved back (moved or deleted since).", "{0} 個は戻せませんでした（移動または削除済み）。", " {0}개는 되돌릴 수 없습니다(이동되었거나 삭제됨).", " {0} konnten nicht zurück (inzwischen verschoben oder gelöscht).", " {0} n’ont pas pu être remis (déplacés ou supprimés depuis).", " {0} no se pudieron devolver (movidos o eliminados).", " {0} não puderam ser devolvidos (movidos ou excluídos).", " {0} не удалось вернуть (перемещены или удалены).", " تعذرت إعادة {0} (نُقلت أو حُذفت).", " {0} वापस नहीं लाई जा सकीं (हटाई या ले जाई गईं)।", " {0}টি ফেরানো যায়নি (সরানো বা মুছে ফেলা হয়েছে)।"]),
    ("sort.undo_title", ["DeskCrate 已撤销归档", "DeskCrate 已復原歸檔", "DeskCrate undid the sort", "DeskCrate が整理を元に戻しました", "DeskCrate가 정리를 실행 취소함", "DeskCrate hat das Aufräumen rückgängig gemacht", "DeskCrate a annulé le rangement", "DeskCrate deshizo la ordenación", "O DeskCrate desfez a organização", "DeskCrate отменил упорядочивание", "تراجع DeskCrate عن الترتيب", "DeskCrate ने व्यवस्था पूर्ववत की", "DeskCrate সাজানো পূর্বাবস্থায় ফিরিয়েছে"]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_complete_and_formats() {
        for (k, v) in TABLE {
            assert!(!v[EN].is_empty(), "{k} lacks English");
            let holes = v[0].matches('{').count();
            for (i, s) in v.iter().enumerate() {
                assert_eq!(s.matches('{').count(), holes, "{k} [{}] placeholder mismatch", LANGS[i]);
            }
        }
        set_language("en-US");
        assert_eq!(tr_n("capsule.count", 3), "3 items");
        assert_eq!(trf("remove.confirm", &["A", "B"]).lines().next(), Some("Delete the grid “A”?"));
        set_language("zh");
        assert_eq!(tr("menu.rename"), "重命名");
        assert_eq!(tr("no.such.key"), "no.such.key");
    }
}
