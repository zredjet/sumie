//! 照合の相手（architecture.md §10）。
//!
//! - resvg 0.48.1（Kura と同じ feature：text だけ）を、Kura と同じ usvg の設定で描く：外の資源を読まない
//!   （`resources_dir: None`、画像の参照はすべて読まない）、ほかは `usvg::Options::default()`（既定の書体 Times New Roman・
//!   12・languages ["en"]・dpi 96）。大きさは Kura と同じく `tree.size().to_int_size()`（f32 の四捨五入・最小 1。ADR-78 (5)）。viewBox がなく幅・高さが
//!   ない（か %）の文書は、usvg 0.48.1 は中身の外接矩形の右下を大きさにする（`width`・`height` のない 50 × 50 の rect は
//!   50 × 50。100 × 100 ではない）。フォントは Kura と同じく `/System/Library/Fonts` とその `Supplemental` の ttf・otf・ttc
//!   を、道の名前の順に登録し、総称の書体は Times・Helvetica・Apple Chancery・Papyrus・Courier
//! - Chrome（`/Applications/Google Chrome.app`）のヘッドレス。SVG を HTML の `<img>` で決めた大きさに置き（画像としての
//!   SVG は外の資源を読まない）、広い窓で撮って左上を切り出す。窓の幅はヘッドレスでは 500 より小さくできないため
//!   （ADR-52）。透明の背景・倍率 1・sRGB・GPU を使わないソフトウェアのラスタ化（`--disable-gpu`。機械の GPU と駆動の層で
//!   縁の値が変わらないように。ADR-57 (2)）。Chrome が SVG を読めないとき（壊れた画像の印を描く）は、`onerror` で窓の右下
//!   （切り出す領域の外）に印の色を塗り、それを見て誤りにする

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
    /// `system`・`none`・フォルダ。フォルダがない（`sytem` のような打ち間違い）なら誤り。
    pub fn parse(s: &str) -> Result<Fonts, String> {
        match s {
            "system" => Ok(Fonts::System),
            "none" => Ok(Fonts::None),
            d if Path::new(d).is_dir() => Ok(Fonts::Dir(PathBuf::from(d))),
            d => Err(format!("--fonts は system・none かフォルダ（{d} がない）")),
        }
    }
}

/// フォルダの ttf・otf・ttc（入れ子は見ない・シンボリックリンクと 1 GiB を超えるものは飛ばす）を道の名前の順に。
/// 拡張子は ASCII の大文字小文字を無視する。並べ方は道の文字列の比べ方（Kura の `fonts.rs` の `build` と同じ）。
pub fn font_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
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

    /// 木を作り、usvg が描く木のために解こうとした画像の参照（`data:` の MIME の型か、外の参照の文字列）を並べる（画像は
    /// 読まない：Kura と同じに、どれも解かずに捨てる）。`Document::needs` の照らし（ADR-63 (1)）に使う。
    pub fn tree_with_images(&self, svg: &[u8]) -> Result<(usvg::Tree, Vec<String>), String> {
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let (a, b) = (seen.clone(), seen.clone());
        let opts = usvg::Options {
            resources_dir: None,
            image_href_resolver: usvg::ImageHrefResolver {
                resolve_data: Box::new(move |mime, _, _| {
                    if let Ok(mut v) = a.lock() {
                        v.push(format!("data:{mime}"));
                    }
                    None
                }),
                resolve_string: Box::new(move |href, _| {
                    if let Ok(mut v) = b.lock() {
                        v.push(href.to_string());
                    }
                    None
                }),
            },
            fontdb: self.db.clone(),
            ..usvg::Options::default()
        };
        let tree = usvg::Tree::from_data(svg, &opts).map_err(|e| format!("usvg：{e}"))?;
        let list = seen.lock().map(|v| v.clone()).unwrap_or_default();
        Ok((tree, list))
    }

    /// 倍率 1 の画素の大きさ（Kura と同じ `to_int_size`）。
    pub fn size(tree: &usvg::Tree) -> (u32, u32) {
        let s = tree.size().to_int_size();
        (s.width(), s.height())
    }

    /// 小数のままの大きさ（Chrome の `<img>` に CSS の画素で渡す）。Chrome は `<img>` を画素の格子に合わせるので、整数で
    /// ない大きさの文書は引き伸ばして描く（resvg は右端の列を半分の被覆で描く）。照合はその文書に印を付ける（ADR-60）。
    pub fn css_size(tree: &usvg::Tree) -> (f32, f32) {
        (tree.size().width(), tree.size().height())
    }

    /// 描く（倍率 `scale`。大きさは幅・高さ × 倍率の四捨五入・最小 1。sumie の `pixel_size` と同じ。ADR-78 (5)）。
    pub fn render(&self, svg: &[u8], scale: f32) -> Result<ResvgOut, String> {
        let tree = self.tree(svg)?;
        let (w, h) = if scale == 1.0 {
            Resvg::size(&tree)
        } else {
            let s = tree.size();
            ((s.width() * scale).round().max(1.0) as u32, (s.height() * scale).round().max(1.0) as u32)
        };
        let mut pixmap = tiny_skia::Pixmap::new(w.max(1), h.max(1)).ok_or("画素の領域を作れない")?;
        resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        let img = Rgba { w: pixmap.width(), h: pixmap.height(), px: pixmap.data().to_vec() };
        Ok(ResvgOut { img, fonts_used: fonts_used(&tree) })
    }
}

/// 木の文字に使われた書体（`ファミリー名（PostScript 名）`）を並べる。pattern・mask・clipPath・フィルタの中の木もたどる。
pub fn fonts_used(tree: &usvg::Tree) -> Vec<String> {
    let db = tree.fontdb();
    let mut out = Vec::new();
    let mut stack: Vec<&usvg::Group> = vec![tree.root()];
    while let Some(g) = stack.pop() {
        if let Some(c) = g.clip_path() {
            stack.push(c.root());
        }
        if let Some(m) = g.mask() {
            stack.push(m.root());
        }
        for f in g.filters() {
            for p in f.primitives() {
                if let usvg::filter::Kind::Image(img) = p.kind() {
                    stack.push(img.root());
                }
            }
        }
        for n in g.children() {
            if let Some(paints) = paints_of(n) {
                for p in paints {
                    if let usvg::Paint::Pattern(pt) = p {
                        stack.push(pt.root());
                    }
                }
            }
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

/// 節の塗りと線の塗り（pattern の中の木をたどるため）。
fn paints_of(n: &usvg::Node) -> Option<Vec<&usvg::Paint>> {
    match n {
        usvg::Node::Path(p) => {
            Some(p.fill().map(|f| f.paint()).into_iter().chain(p.stroke().map(|s| s.paint())).collect())
        }
        _ => None,
    }
}

/// Chrome の照合の相手。
pub struct Chrome {
    pub exe: PathBuf,
    pub version: String,
}

/// Chrome の場所（macOS）。
pub const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

/// Chrome の 1 回の撮影の時間の上限（重い入力で止まったままにしない）。
const CHROME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// 読めなかった印の色（窓の右下の画素。切り出す領域の外）。
const LOAD_ERROR_MARK: [u8; 4] = [255, 0, 255, 255];

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

    /// `svg` を CSS の画素で `css_w × css_h`（小数のまま）に置いて撮り、左上の `w × h` を切り出す（`work` は使い捨ての
    /// 作業のフォルダ。並べて回すときは別々にする）。返すのは絵と、撮るのにかかった回数（撮れない・時間切れは 1 度だけ
    /// やり直す。並べて回すと、まれに Chrome が絵を書かずに終わるか止まる）。
    pub fn render(&self, svg: &Path, w: u32, h: u32, css: (f32, f32), work: &Path) -> Result<(Rgba, u32), String> {
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
                 </head><body><img src=\"{url}\" style=\"width:{cw}px;height:{ch}px\" \
                 onerror=\"this.style.display='none';document.documentElement.style.background='rgb(255,0,255)'\">\
                 </body></html>",
                cw = css.0,
                ch = css.1
            ),
        )
        .map_err(|e| e.to_string())?;
        let shot = work.join("shot.png");
        let _ = std::fs::remove_file(&shot);
        // 窓は切り出す領域より 1 画素ずつ大きくして、右下の画素を読めなかった印に使う
        let (ww, wh) = (w.saturating_add(1).max(800), h.saturating_add(1).max(600));
        let mut attempts = 0u32;
        let mut timed_out = false;
        for attempt in 0..2 {
            attempts = attempt + 1;
            timed_out = false;
            if attempt > 0 {
                let _ = std::fs::remove_dir_all(work.join("profile"));
            }
            let mut child = Command::new(&self.exe)
                .args([
                    "--headless",
                    "--disable-gpu",
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
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| format!("Chrome を起動できない：{e}"))?;
            let start = std::time::Instant::now();
            loop {
                if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                    break;
                }
                if start.elapsed() > CHROME_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            if shot.exists() {
                break;
            }
        }
        if timed_out {
            return Err(format!("Chrome が {} 秒で終わらない（{attempts} 回）", CHROME_TIMEOUT.as_secs()));
        }
        if !shot.exists() {
            return Err(format!("Chrome が撮れなかった（{attempts} 回）"));
        }
        let full = image::read_png(&shot)?;
        if full.w < ww || full.h < wh {
            return Err(format!("撮った絵が窓より小さい（{}×{}、窓 {ww}×{wh}）", full.w, full.h));
        }
        let corner = ((full.h as usize - 1) * full.w as usize + full.w as usize - 1) * 4;
        if full.px.get(corner..corner + 4) == Some(&LOAD_ERROR_MARK[..]) {
            return Err("Chrome が SVG を読めなかった（壊れた画像）".into());
        }
        Ok((full.crop(w, h), attempts))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_files_are_sorted_by_path_and_skip_links_and_other_files() {
        let dir = std::env::temp_dir().join(format!("sumie-fonts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for n in ["b.ttf", "A.TTC", "a.otf", "c.txt", "sub/d.ttf"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        std::os::unix::fs::symlink(dir.join("b.ttf"), dir.join("link.ttf")).unwrap();
        let got: Vec<String> = font_files(std::slice::from_ref(&dir))
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        // 道の文字列の順（大文字が先）・入れ子とシンボリックリンクと別の拡張子は飛ばす
        assert_eq!(got, ["A.TTC", "a.otf", "b.ttf"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn m0_sizes_and_images_are_not_read() {
        let resvg = Resvg::new(&Fonts::None);
        let dir = crate::corpus::workspace_root().join("tests/svg/m0");
        let mut got = Vec::new();
        for n in ["no-size.svg", "rect.svg", "stroke-circle.svg", "viewbox-only.svg"] {
            let t = resvg.tree(&std::fs::read(dir.join(n)).unwrap()).unwrap();
            got.push((n, Resvg::size(&t)));
        }
        // 大きさのない文書は中身の外接矩形（ADR-58 (1)）
        assert_eq!(
            got,
            [
                ("no-size.svg", (50, 50)),
                ("rect.svg", (64, 48)),
                ("stroke-circle.svg", (64, 64)),
                ("viewbox-only.svg", (30, 20))
            ]
        );
        // Kura の設定は画像の参照を読まない（data: の PNG も描かない）
        let png = {
            let p = std::env::temp_dir().join(format!("sumie-1px-{}.png", std::process::id()));
            crate::image::write_png(&p, &Rgba { w: 1, h: 1, px: vec![255, 0, 0, 255] }).unwrap();
            let b = std::fs::read(&p).unwrap();
            let _ = std::fs::remove_file(&p);
            b
        };
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"4\" height=\"4\"><image width=\"4\" height=\"4\" href=\"data:image/png;base64,{}\"/></svg>",
            base64(&png)
        );
        let out = resvg.render(svg.as_bytes(), 1.0).unwrap();
        assert!(out.img.px.iter().all(|&b| b == 0), "画像を描いた");
        assert_eq!(file_url(Path::new("/a b/é.svg")), "file:///a%20b/%C3%A9.svg");
    }

    fn base64(b: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in b.chunks(3) {
            let n = (u32::from(c[0]) << 16)
                | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                | u32::from(*c.get(2).unwrap_or(&0));
            for k in 0..4 {
                if k <= c.len() {
                    s.push(T[((n >> (18 - 6 * k)) & 63) as usize] as char);
                } else {
                    s.push('=');
                }
            }
        }
        s
    }

    /// Kura の設定の resvg（raster-images なし）は、絵文字に Apple Color Emoji を選び、何も描かない（送りの幅だけ空く。
    /// ADR-56 (2) を決めた確かめ）。システムのフォントのある macOS で手で回す：`cargo test -p sumie-tools -- --ignored emoji`
    #[test]
    #[ignore]
    fn emoji_is_not_drawn_by_resvg() {
        let resvg = Resvg::new(&Fonts::System);
        let svg = std::fs::read(crate::corpus::workspace_root().join("tests/svg/m0/emoji.svg")).unwrap();
        let out = resvg.render(&svg, 1.0).unwrap();
        assert!(out.fonts_used.iter().any(|f| f.starts_with("Apple Color Emoji")), "{:?}", out.fonts_used);
        // 「A」の右（x ≥ 24）は透明のまま
        let (w, h) = (out.img.w as usize, out.img.h as usize);
        let drawn = (0..h).flat_map(|y| (24..w).map(move |x| (y * w + x) * 4 + 3)).any(|i| out.img.px[i] != 0);
        assert!(!drawn, "絵文字の所に何か描いた");
        // 「A」は描く
        assert!(out.img.px.chunks(4).any(|p| p[3] != 0));
    }

    /// Chrome が読めない SVG を誤りにする（Chrome のある macOS で手で回す：`cargo test -p sumie-tools -- --ignored chrome`）。
    #[test]
    #[ignore]
    fn chrome_marks_unreadable_svg() {
        let chrome = Chrome::find().unwrap();
        let dir = std::env::temp_dir().join(format!("sumie-chrome-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.svg");
        std::fs::write(&bad, b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"40\"><rect").unwrap();
        let e = chrome.render(&bad, 40, 40, (40.0, 40.0), &dir.join("w1")).unwrap_err();
        assert!(e.contains("読めなかった"), "{e}");
        let good = dir.join("good.svg");
        std::fs::write(&good, b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"40\"><rect width=\"40\" height=\"40\" fill=\"#f0f\"/></svg>").unwrap();
        // 全面が印の色でも、切り出す領域の外の画素は透明なので誤りにしない
        let (img, _) = chrome.render(&good, 40, 40, (40.0, 40.0), &dir.join("w2")).unwrap();
        assert_eq!(&img.px[0..4], &[255, 0, 255, 255]);
        // 小数の大きさも撮れる（Chrome は <img> を画素の格子に合わせて引き伸ばすので、値は照合で印を付けて見る）
        let half = dir.join("half.svg");
        std::fs::write(&half, b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10.5\" height=\"4\"><rect width=\"10.5\" height=\"4\"/></svg>").unwrap();
        let (img, _) = chrome.render(&half, 11, 4, (10.5, 4.0), &dir.join("w3")).unwrap();
        assert_eq!((img.w, img.h), (11, 4));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
