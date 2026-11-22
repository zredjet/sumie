//! 照合の相手（architecture.md §10）。
//!
//! - resvg 0.48.1（Kura と同じ feature：text だけ）を、Kura と同じ usvg の設定で描く：外の資源を読まない
//!   （`resources_dir: None`、画像の参照はすべて読まない）、ほかは `usvg::Options::default()`（既定の書体 Times New Roman・
//!   12・languages ["en"]・dpi 96・大きさのない文書は 100 × 100）。フォントは Kura と同じく `/System/Library/Fonts` と
//!   その `Supplemental` の ttf・otf・ttc を、道の名前の順に登録し、総称の書体は Times・Helvetica・Apple Chancery・
//!   Papyrus・Courier。大きさは Kura と同じく `tree.size().to_int_size()`（切り上げ）
//! - Chrome（`/Applications/Google Chrome.app`）のヘッドレス。SVG を HTML の `<img>` で決めた大きさに置き（画像としての
//!   SVG は外の資源を読まない）、広い窓で撮って左上を切り出す。窓の幅はヘッドレスでは 500 より小さくできないため
//!   （ADR-52）。透明の背景・倍率 1・sRGB

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use resvg::tiny_skia;
use resvg::usvg::{self, fontdb};

use crate::image::{self, Rgba};

/// Kura の総称の書体（WebKit の macOS の既定。Kura の `kura_mac::fonts::macos_generic_families`）。
pub const KURA_GENERIC: [(&str, &str); 5] = [
    ("serif", "Times"),
    ("sans-serif", "Helvetica"),
    ("cursive", "Apple Chancery"),
    ("fantasy", "Papyrus"),
    ("monospace", "Courier"),
];

/// Kura のシステムのフォントのフォルダ（入れ子は見ない）。
pub const SYSTEM_FONT_DIRS: [&str; 2] = ["/System/Library/Fonts", "/System/Library/Fonts/Supplemental"];

/// フォントの選び方。
#[derive(Debug, Clone)]
pub enum Fonts {
    /// Kura と同じシステムのフォント
    System,
    /// フォントなし
    None,
    /// フォルダの中のフォント（名前の順。総称の書体は Kura と同じ）
    Dir(PathBuf),
}

impl Fonts {
    pub fn parse(s: &str) -> Fonts {
        match s {
            "system" => Fonts::System,
            "none" => Fonts::None,
            d => Fonts::Dir(PathBuf::from(d)),
        }
    }
}

/// フォルダの ttf・otf・ttc（入れ子は見ない・シンボリックリンクと 1 GiB を超えるものは飛ばす）を道の名前の順に。
fn font_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
            let ext = p.extension().map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if m.is_file() && m.len() <= 1 << 30 && matches!(ext.as_str(), "ttf" | "otf" | "ttc") {
                out.push(p);
            }
        }
    }
    out.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    out
}

/// fontdb の集まりを作る（Kura の `SvgFonts::new` と同じ順と総称の書体）。
pub fn font_db(fonts: &Fonts) -> fontdb::Database {
    let mut db = fontdb::Database::new();
    db.set_serif_family(KURA_GENERIC[0].1);
    db.set_sans_serif_family(KURA_GENERIC[1].1);
    db.set_cursive_family(KURA_GENERIC[2].1);
    db.set_fantasy_family(KURA_GENERIC[3].1);
    db.set_monospace_family(KURA_GENERIC[4].1);
    let dirs: Vec<PathBuf> = match fonts {
        Fonts::System => SYSTEM_FONT_DIRS.iter().map(PathBuf::from).collect(),
        Fonts::None => Vec::new(),
        Fonts::Dir(d) => vec![d.clone()],
    };
    for p in font_files(&dirs) {
        if let Ok(bytes) = std::fs::read(&p) {
            db.load_font_source(fontdb::Source::Binary(Arc::new(bytes)));
        }
    }
    db
}

/// resvg の照合の相手。
pub struct Resvg {
    db: Arc<fontdb::Database>,
}

/// 描いた結果と、文字に使った書体（書体名の並び。重ならない）。
pub struct ResvgOut {
    pub img: Rgba,
    pub fonts_used: Vec<String>,
}

impl Resvg {
    pub fn new(fonts: &Fonts) -> Resvg {
        Resvg { db: Arc::new(font_db(fonts)) }
    }

    pub fn faces(&self) -> usize {
        self.db.len()
    }

    /// Kura と同じ usvg の設定。
    fn options(&self) -> usvg::Options<'static> {
        usvg::Options {
            resources_dir: None,
            image_href_resolver: usvg::ImageHrefResolver {
                resolve_data: Box::new(|_, _, _| None),
                resolve_string: Box::new(|_, _| None),
            },
            fontdb: self.db.clone(),
            ..usvg::Options::default()
        }
    }

    /// 木を作る（大きさを読むのにも使う）。
    pub fn tree(&self, svg: &[u8]) -> Result<usvg::Tree, String> {
        usvg::Tree::from_data(svg, &self.options()).map_err(|e| format!("usvg：{e}"))
    }

    /// 倍率 1 の画素の大きさ（Kura と同じ `to_int_size`）。
    pub fn size(tree: &usvg::Tree) -> (u32, u32) {
        let s = tree.size().to_int_size();
        (s.width(), s.height())
    }

    /// 描く（倍率 `scale`。大きさは幅・高さ × 倍率の切り上げ）。
    pub fn render(&self, svg: &[u8], scale: f32) -> Result<ResvgOut, String> {
        let tree = self.tree(svg)?;
        let (w, h) = if scale == 1.0 {
            Resvg::size(&tree)
        } else {
            let s = tree.size();
            ((s.width() * scale).ceil() as u32, (s.height() * scale).ceil() as u32)
        };
        let mut pixmap = tiny_skia::Pixmap::new(w.max(1), h.max(1)).ok_or("画素の領域を作れない")?;
        resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        let img = Rgba { w: pixmap.width(), h: pixmap.height(), px: pixmap.data().to_vec() };
        Ok(ResvgOut { img, fonts_used: fonts_used(&tree) })
    }
}

/// 木の文字に使われた書体（`ファミリー名（PostScript 名）`）を並べる。
pub fn fonts_used(tree: &usvg::Tree) -> Vec<String> {
    let db = tree.fontdb();
    let mut out = Vec::new();
    let mut stack: Vec<&usvg::Group> = vec![tree.root()];
    while let Some(g) = stack.pop() {
        for n in g.children() {
            match n {
                usvg::Node::Group(c) => stack.push(c),
                usvg::Node::Text(t) => {
                    for span in t.layouted() {
                        for gl in &span.positioned_glyphs {
                            let name = db
                                .face(gl.font)
                                .map(|f| {
                                    let fam = f.families.first().map(|x| x.0.clone()).unwrap_or_default();
                                    format!("{fam}（{}）", f.post_script_name)
                                })
                                .unwrap_or_else(|| "?".into());
                            if !out.contains(&name) {
                                out.push(name);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Chrome の照合の相手。
pub struct Chrome {
    pub exe: PathBuf,
    pub version: String,
}

/// Chrome の場所（macOS）。
pub const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

impl Chrome {
    pub fn find() -> Result<Chrome, String> {
        let exe = PathBuf::from(CHROME);
        if !exe.exists() {
            return Err(format!("Chrome がない：{CHROME}"));
        }
        let o = Command::new(&exe).arg("--version").output().map_err(|e| e.to_string())?;
        let version = String::from_utf8_lossy(&o.stdout).trim().to_string();
        Ok(Chrome { exe, version })
    }

    /// `svg` を `w × h` の大きさで描く（`work` は使い捨ての作業のフォルダ。並べて回すときは別々にする）。
    pub fn render(&self, svg: &Path, w: u32, h: u32, work: &Path) -> Result<Rgba, String> {
        std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
        // Chrome に渡す URL は絶対の道で作る
        let work = work.canonicalize().map_err(|e| format!("{}: {e}", work.display()))?;
        let work = work.as_path();
        let svg = svg.canonicalize().map_err(|e| format!("{}: {e}", svg.display()))?;
        let html = work.join("page.html");
        let url = file_url(&svg);
        std::fs::write(
            &html,
            format!(
                "<!doctype html><html><head><meta charset=\"utf-8\"><style>html,body{{margin:0;padding:0;\
                 background:transparent;overflow:hidden}}img{{display:block;position:absolute;left:0;top:0}}</style>\
                 </head><body><img src=\"{url}\" width=\"{w}\" height=\"{h}\"></body></html>"
            ),
        )
        .map_err(|e| e.to_string())?;
        let shot = work.join("shot.png");
        let _ = std::fs::remove_file(&shot);
        let (ww, wh) = (w.max(800), h.max(600));
        let out = Command::new(&self.exe)
            .args([
                "--headless",
                "--hide-scrollbars",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-extensions",
                "--disable-background-networking",
                "--default-background-color=00000000",
                "--force-device-scale-factor=1",
                "--force-color-profile=srgb",
                "--allow-file-access-from-files",
            ])
            .arg(format!("--user-data-dir={}", work.join("profile").display()))
            .arg(format!("--window-size={ww},{wh}"))
            .arg(format!("--screenshot={}", shot.display()))
            .arg(file_url(&html))
            .output()
            .map_err(|e| format!("Chrome を起動できない：{e}"))?;
        if !shot.exists() {
            return Err(format!("Chrome が撮れなかった：{}", String::from_utf8_lossy(&out.stderr)));
        }
        Ok(image::read_png(&shot)?.crop(w, h))
    }
}

/// ファイルの URL（空白などを % にする）。
fn file_url(p: &Path) -> String {
    let mut s = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}
