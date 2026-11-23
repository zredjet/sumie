//! CSS のプロパティの機械の洗い出し（単位 3 の 5 回目のレビューの重大 8 とユーザーの判断「機械で洗い出す」）。
//!
//! Chrome が知るどのプロパティ（計算した値の一覧と `style` の名前。まとめも）にも、決めた値の一覧のうち Chrome が受ける値
//! （`CSS.supports`）を、根の svg・g・図形・use・入れ子の svg に書いた SVG を、Chrome のヘッドレスの中で img として描き、
//! 書かないものと画素が違う（Chrome の絵を変える）組を集める。その組を sumie で描き、断る（NotYet などの安全な断り）か、
//! Chrome とおおよそ同じ絵か、違う絵（黙って違う絵。直すか NotYet にする候補）かに分けて書く。
//!
//! 出力：`<out>/report.txt`（違う絵の組をプロパティごとに）・`<out>/all.tsv`（すべての組）・`<out>/svg/`（違う絵の SVG と、
//! Chrome と sumie の PNG）。

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::Command;

use crate::compare_sumie::{Drawn, render_sumie};
use crate::corpus::{arg, workspace_root};
use crate::image::{Rgba, premultiply, write_png};
use crate::oracle::Chrome;

/// 絵の 1 辺（CSS の画素）。
const SIDE: u32 = 64;

/// Chrome で走らせる頁（`__POOL__` を値の一覧に置き換える）。結果は `<pre id="out">` に 1 行ずつ：
/// `B\t<位置>\t<SVG の base64>\t<画素の base64>`（書かないもの）と、
/// `A\t<プロパティ>\t<位置>\t<値の base64>\t<違う画素の数>\t<SVG の base64>\t<画素の base64>`（絵を変える組）。
const PAGE: &str = r###"<!doctype html><html><head><meta charset="utf-8"></head><body><pre id="out">pending</pre><script>
const SIDE = __SIDE__;
const POOL = __POOL__;
const DEFS = '<defs><filter id="f"><feFlood flood-color="red"/></filter><mask id="m"><rect width="20" height="20" fill="white"/></mask>'
  + '<clipPath id="c"><rect width="20" height="20"/></clipPath><marker id="mk" markerWidth="4" markerHeight="4"><rect width="4" height="4" fill="red"/></marker>'
  + '<pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="2" height="2" fill="red"/></pattern>'
  + '<linearGradient id="lg"><stop stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient></defs>';
function content(st) {
  const s = st === null ? '' : " style='" + st + "'";
  return '<rect x="6" y="6" width="24" height="20" fill="#36c" stroke="#c63" stroke-width="3"' + s + '/>'
    + '<path d="M36 8 L58 30 L36 52" fill="#3a3" stroke="#333" stroke-width="2"' + s + '/>'
    + '<circle cx="18" cy="46" r="10" fill="#a3a"/>';
}
const HEAD = '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="' + SIDE + '" height="' + SIDE + '"';
const POS = {
  root: st => HEAD + (st === null ? '' : " style='" + st + "'") + '>' + DEFS + content(null) + '</svg>',
  g: st => HEAD + '>' + DEFS + '<g' + (st === null ? '' : " style='" + st + "'") + '>' + content(null) + '</g></svg>',
  shape: st => HEAD + '>' + DEFS + content(st) + '</svg>',
  use: st => HEAD + '>' + DEFS + '<defs><g id="u">' + content(null) + '</g></defs><use href="#u"' + (st === null ? '' : " style='" + st + "'") + '/></svg>',
  nested: st => HEAD + '>' + DEFS + '<svg x="0" y="0" width="' + SIDE + '" height="' + SIDE + '"' + (st === null ? '' : " style='" + st + "'") + '>' + content(null) + '</svg></svg>',
};
function esc(v) { return v.replace(/&/g, '&amp;').replace(/'/g, '&apos;').replace(/</g, '&lt;'); }
function b64(bytes) { let s = ''; for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000)); return btoa(s); }
function b64s(str) { return b64(new TextEncoder().encode(str)); }
function load(svg) {
  return new Promise(res => { const i = new Image(); i.onload = () => res(i); i.onerror = () => res(null); i.src = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(svg); });
}
const canvas = document.createElement('canvas'); canvas.width = SIDE; canvas.height = SIDE;
const ctx = canvas.getContext('2d', { willReadFrequently: true });
async function px(svg) {
  const i = await load(svg);
  ctx.clearRect(0, 0, SIDE, SIDE);
  if (i) ctx.drawImage(i, 0, 0);
  return { ok: !!i, data: new Uint8Array(ctx.getImageData(0, 0, SIDE, SIDE).data) };
}
(async () => {
  const out = [];
  try {
    const names = new Set([...getComputedStyle(document.documentElement)]);
    for (const k in document.body.style) {
      if (typeof document.body.style[k] !== 'string' || /^[0-9]/.test(k) || k === 'cssText' || k === 'cssFloat') continue;
      const n = k.replace(/[A-Z]/g, c => '-' + c.toLowerCase()).replace(/^webkit-/, '-webkit-');
      if (CSS.supports(n, 'initial')) names.add(n);
    }
    const base = {};
    for (const p in POS) {
      const svg = POS[p](null);
      base[p] = await px(svg);
      out.push(['B', p, b64s(svg), b64(base[p].data)].join('\t'));
    }
    let tried = 0;
    for (const n of [...names].sort()) {
      if (n.startsWith('--')) continue;
      for (const v of POOL) {
        if (!CSS.supports(n, v)) continue;
        for (const p in POS) {
          const svg = POS[p](n + ':' + esc(v));
          const r = await px(svg);
          tried++;
          let d = 0;
          const a = r.data, b = base[p].data;
          for (let k = 0; k < a.length; k += 4) {
            if (a[k] !== b[k] || a[k + 1] !== b[k + 1] || a[k + 2] !== b[k + 2] || a[k + 3] !== b[k + 3]) d++;
          }
          if (d > 0 || !r.ok) out.push(['A', n, p, b64s(v), r.ok ? d : -1, b64s(svg), b64(a)].join('\t'));
        }
      }
    }
    out.push('T\t' + names.size + '\t' + tried);
  } catch (e) {
    out.push('E\t' + e);
  }
  document.getElementById('out').textContent = out.join('\n');
})();
</script></body></html>"###;

/// 試す値（Chrome が受けるものだけを使う。全体の言葉は除く）。
const POOL: &[&str] = &[
    "none",
    "auto",
    "normal",
    "hidden",
    "visible",
    "scroll",
    "clip",
    "collapse",
    "block",
    "inline",
    "inline-block",
    "contents",
    "flex",
    "grid",
    "table",
    "list-item",
    "flow-root",
    "ruby",
    "absolute",
    "relative",
    "fixed",
    "sticky",
    "static",
    "bold",
    "italic",
    "oblique",
    "small-caps",
    "underline",
    "overline",
    "line-through",
    "uppercase",
    "capitalize",
    "nowrap",
    "pre",
    "pre-wrap",
    "break-all",
    "break-word",
    "anywhere",
    "keep-all",
    "center",
    "left",
    "right",
    "top",
    "bottom",
    "middle",
    "start",
    "end",
    "justify",
    "baseline",
    "stretch",
    "space-between",
    "both",
    "horizontal-tb",
    "vertical-rl",
    "vertical-lr",
    "sideways-rl",
    "rtl",
    "ltr",
    "embed",
    "isolate",
    "isolate-override",
    "bidi-override",
    "plaintext",
    "ellipsis",
    "solid",
    "dashed",
    "dotted",
    "double",
    "groove",
    "ridge",
    "inset",
    "outset",
    "multiply",
    "screen",
    "overlay",
    "difference",
    "exclusion",
    "luminosity",
    "color",
    "hue",
    "saturation",
    "luminance",
    "alpha",
    "match-source",
    "round",
    "bevel",
    "miter",
    "miter-clip",
    "arcs",
    "square",
    "butt",
    "evenodd",
    "nonzero",
    "optimizeSpeed",
    "optimizeLegibility",
    "crispEdges",
    "geometricPrecision",
    "pixelated",
    "smooth",
    "high-quality",
    "non-scaling-stroke",
    "fill",
    "stroke",
    "markers",
    "paint",
    "view-box",
    "fill-box",
    "stroke-box",
    "border-box",
    "content-box",
    "padding-box",
    "margin-box",
    "text",
    "preserve-3d",
    "flat",
    "strict",
    "content",
    "size",
    "layout",
    "style",
    "inline-size",
    "sRGB",
    "linearRGB",
    "repeat",
    "no-repeat",
    "repeat-x",
    "space",
    "cover",
    "contain",
    "dark",
    "light",
    "only light",
    "grayscale",
    "antialiased",
    "mixed",
    "upright",
    "sideways",
    "currentColor",
    "transparent",
    "red",
    "lime",
    "rgb(255 0 0 / 50%)",
    "#0000ff80",
    "0",
    "1",
    "2",
    "3",
    "0.5",
    "-1",
    "10",
    "100",
    "45deg",
    "90deg",
    "180deg",
    "0.5turn",
    "0px",
    "1px",
    "3px",
    "5px",
    "10px",
    "-10px",
    "20px",
    "50%",
    "25%",
    "-50%",
    "100%",
    "200%",
    "2em",
    "5vw",
    "1.5",
    "2px 2px",
    "10px 10px",
    "5px 10px 15px",
    "10px 10px 10px 10px",
    "2px solid red",
    "5px dashed lime",
    "10px solid rgb(0 0 255 / 50%)",
    "2px 2px red",
    "5px 5px 0 red",
    "0 0 5px red",
    "inset 0 0 10px red",
    "10px 10px 5px 2px red",
    "url(#x)",
    "url(#f)",
    "url(#m)",
    "url(#c)",
    "url(#mk)",
    "url(#p)",
    "url(#lg)",
    "blur(2px)",
    "drop-shadow(2px 2px red)",
    "grayscale(1)",
    "invert(1)",
    "opacity(0.5)",
    "sepia(1)",
    "hue-rotate(90deg)",
    "brightness(2)",
    "contrast(2)",
    "saturate(3)",
    "translate(10px)",
    "translate(10px, 10px)",
    "rotate(45deg)",
    "scale(2)",
    "scale(0.5)",
    "skewX(20deg)",
    "matrix(1,0,0,1,10,10)",
    "perspective(100px)",
    "rotateX(45deg)",
    "translateZ(10px)",
    "inset(10px)",
    "circle(10px)",
    "circle(50%)",
    "ellipse(10px 20px)",
    "polygon(0 0, 30px 0, 0 30px)",
    "path(\"M0 0 L30 0 L0 30 Z\")",
    "xywh(0 0 20px 20px)",
    "rect(0 20px 20px 0)",
    "rect(0, 20px, 20px, 0)",
    "linear-gradient(red, blue)",
    "radial-gradient(red, blue)",
    "conic-gradient(red, blue)",
    "repeating-linear-gradient(red 0 5px, blue 5px 10px)",
    "image(red)",
    "url(a.png)",
    "\"a\"",
    "counter(x)",
    "attr(id)",
    "x",
    "a",
    "auto auto",
    "1 1 auto",
    "row",
    "column",
    "wrap",
    "dense",
    "span 2",
    "1fr",
    "1fr 1fr",
    "repeat(2, 1fr)",
    "paused",
    "running",
    "infinite",
    "alternate",
    "forwards",
    "ease",
    "linear",
    "steps(2)",
    "1s",
    "100ms",
    "all",
    "opacity",
    "transform",
    "instant",
    "manual",
    "break-spaces",
    "pretty",
    "balance",
    "stable",
    "discard",
    "avoid",
    "page",
    "always",
    "from-image",
    "0 0",
    "left top",
    "right bottom",
    "center center",
    "10px 20px",
    "scroll-position",
    "over",
    "under",
    "alphabetic",
    "ideographic",
    "central",
    "mathematical",
    "hanging",
    "text-top",
    "text-bottom",
    "sub",
    "super",
    "all-scroll",
    "pointer",
    "crosshair",
    "visiblePainted",
    "visibleFill",
    "visibleStroke",
    "painted",
    "bounding-box",
    "1/1",
    "16/9",
    "10px 10px 10px",
    "border-area",
    "span",
    "first",
    "last",
    "force-end",
    "allow-end",
    "mandatory",
    "proximity",
    "x mandatory",
    "1 1",
    "1 0",
    "0 1",
    "wavy",
    "wavy red",
    "from-font",
    "10px red",
    "ruby-base",
    "hidden visible",
    "visible hidden",
    "clip clip",
    "10px 20px 30px 40px",
    "0 0 0 0",
    "10px 10px 10px red",
    "-10px -10px",
    "50% 50%",
    "0% 0%",
    "100% 100%",
    "bottom right",
    "2",
    "0.25",
    "4px 2px",
    "5 5",
    "10",
    "3 1",
    "4",
    "evenodd red",
    "0.2",
    "50",
    "200",
    "1000",
    "smaller",
    "larger",
    "xx-large",
    "xx-small",
    "8px",
    "40px",
];

/// 1 つの組の sumie の結果。
enum Verdict {
    /// 断った（NotYet・描かない機能・ほかの断り）
    Refused(String),
    /// Chrome とおおよそ同じ
    Same,
    /// Chrome と違う（違う画素の数・sumie が書かないものと同じ絵か）
    Differs(usize, bool),
}

/// base64 を読む（標準の字の並び。= の詰めを許す）。
fn unb64(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &c in s.as_bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return Err(format!("base64 でない字 {c}")),
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// dump-dom の文字の参照を戻す（`<pre>` の中身は &・<・> だけが参照になる）。
fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// Chrome のキャンバスの画素（乗算していない）を乗算済みにする。
fn chrome_rgba(px: &[u8]) -> Rgba {
    let mut out = Vec::with_capacity(px.len());
    for c in px.as_chunks::<4>().0 {
        let a = c[3];
        out.extend_from_slice(&[premultiply(c[0], a), premultiply(c[1], a), premultiply(c[2], a), a]);
    }
    Rgba { w: SIDE, h: SIDE, px: out }
}

/// どれかのチャンネルが `tol` を超えて違う画素の数（大きさが違えば全部）。
fn diff(a: &Rgba, b: &Rgba, tol: u8) -> usize {
    if a.w != b.w || a.h != b.h {
        return (SIDE * SIDE) as usize;
    }
    a.px.as_chunks::<4>()
        .0
        .iter()
        .zip(b.px.as_chunks::<4>().0.iter())
        .filter(|(x, y)| x.iter().zip(y.iter()).any(|(p, q)| p.abs_diff(*q) > tol))
        .count()
}

pub fn run(args: &[String]) -> Result<(), String> {
    crate::corpus::check_args(args, &["--out"], &[], 0)?;
    let ws = workspace_root();
    let out_dir = arg(args, "--out").map(PathBuf::from).unwrap_or_else(|| ws.join("local/css-sweep"));
    let work = out_dir.join("work");
    let _ = std::fs::remove_dir_all(out_dir.join("svg"));
    std::fs::create_dir_all(out_dir.join("svg")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let chrome = Chrome::find()?;
    // 頁を書いて Chrome で走らせる
    let mut pool = String::from("[");
    for v in POOL {
        let _ = write!(pool, "{},", json_str(v));
    }
    pool.push(']');
    let page = PAGE.replace("__SIDE__", &SIDE.to_string()).replace("__POOL__", &pool);
    let html = work.canonicalize().map_err(|e| e.to_string())?.join("page.html");
    std::fs::write(&html, page).map_err(|e| e.to_string())?;
    eprintln!("Chrome（{}）で洗い出す…", chrome.version);
    let o = Command::new(&chrome.exe)
        .args([
            "--headless",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--disable-background-networking",
            "--force-color-profile=srgb",
            "--virtual-time-budget=3600000",
            "--dump-dom",
        ])
        .arg(format!("--user-data-dir={}", work.join("profile").display()))
        .arg(format!("file://{}", html.display()))
        .output()
        .map_err(|e| format!("Chrome を起動できない：{e}"))?;
    let dom = String::from_utf8_lossy(&o.stdout);
    let start = dom.find("<pre id=\"out\">").ok_or("結果の pre がない")? + "<pre id=\"out\">".len();
    let end = dom[start..].find("</pre>").ok_or("結果の pre が閉じない")? + start;
    let text = unescape(&dom[start..end]);
    // 書かないものの絵（位置ごと）と、sumie の書かないものの絵
    let mut base: Vec<(String, Rgba, Option<Rgba>)> = Vec::new();
    let mut rows: Vec<(String, String, String, i64, Vec<u8>, Rgba)> = Vec::new();
    let mut total = String::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        match f.first().copied() {
            Some("B") if f.len() == 4 => {
                let svg = unb64(f[2])?;
                let sumie = match render_sumie(&svg, 1) {
                    Drawn::Image(img) => Some(img),
                    _ => None,
                };
                base.push((f[1].to_string(), chrome_rgba(&unb64(f[3])?), sumie));
            }
            Some("A") if f.len() == 7 => {
                let value = String::from_utf8_lossy(&unb64(f[3])?).into_owned();
                let d: i64 = f[4].parse().map_err(|e| format!("{e}"))?;
                rows.push((f[1].to_string(), f[2].to_string(), value, d, unb64(f[5])?, chrome_rgba(&unb64(f[6])?)));
            }
            Some("T") => total = f.get(1..).map(|x| x.join(" 組の値を試した組 ")).unwrap_or_default(),
            Some("E") => return Err(format!("頁の誤り：{line}")),
            _ => {}
        }
    }
    eprintln!("プロパティ {total}・絵を変える組 {}", rows.len());
    // 位置ごとの、sumie と Chrome の書かないものどうしの差（反エイリアスの差の目安）
    let base_gap = |pos: &str| -> usize {
        base.iter().find(|b| b.0 == pos).and_then(|b| b.2.as_ref().map(|s| diff(s, &b.1, 16))).unwrap_or(0)
    };
    let mut all = String::from("プロパティ\t位置\t値\tChrome の違う画素\tsumie\n");
    let mut bad: Vec<(String, String, String, Verdict)> = Vec::new();
    let (mut n_ref, mut n_same, mut n_diff) = (0usize, 0usize, 0usize);
    for (i, (prop, pos, value, d, svg, chrome_img)) in rows.iter().enumerate() {
        let verdict = match render_sumie(svg, 1) {
            Drawn::Excluded(r) | Drawn::Refused(r) => Verdict::Refused(r),
            Drawn::Image(img) => {
                let gap = diff(&img, chrome_img, 16);
                if gap <= base_gap(pos) + 8 {
                    Verdict::Same
                } else {
                    let ignored =
                        base.iter().find(|b| &b.0 == pos).and_then(|b| b.2.as_ref()).is_some_and(|s| s.px == img.px);
                    let name = format!("{i:04}-{}-{pos}", prop.trim_start_matches('-'));
                    let _ = std::fs::write(out_dir.join("svg").join(format!("{name}.svg")), svg);
                    let _ = write_png(&out_dir.join("svg").join(format!("{name}-chrome.png")), chrome_img);
                    let _ = write_png(&out_dir.join("svg").join(format!("{name}-sumie.png")), &img);
                    Verdict::Differs(gap, ignored)
                }
            }
        };
        let s = match &verdict {
            Verdict::Refused(r) => {
                n_ref += 1;
                format!("断る：{r}")
            }
            Verdict::Same => {
                n_same += 1;
                "同じ".to_string()
            }
            Verdict::Differs(g, ign) => {
                n_diff += 1;
                format!("違う：{g} 画素{}", if *ign { "（書かないものと同じ絵：黙って無視）" } else { "" })
            }
        };
        let _ = writeln!(all, "{prop}\t{pos}\t{value}\t{d}\t{s}");
        if matches!(verdict, Verdict::Differs(..)) {
            bad.push((prop.clone(), pos.clone(), value.clone(), verdict));
        }
    }
    let mut report = format!(
        "CSS のプロパティの洗い出し（Chrome {}・{} × {} 画素）\nプロパティ {total}\n絵を変える組 {}：sumie が断る {n_ref}・同じ {n_same}・違う {n_diff}\n\n",
        chrome.version,
        SIDE,
        SIDE,
        rows.len()
    );
    report.push_str("位置ごとの書かないものの sumie と Chrome の差（16/255 を超える画素）：");
    for b in &base {
        let _ = write!(report, "{} {}・", b.0, base_gap(&b.0));
    }
    report.push_str("\n\n## 違う絵の組（プロパティごと）\n");
    let mut last = String::new();
    for (prop, pos, value, v) in &bad {
        if *prop != last {
            let _ = writeln!(report, "\n{prop}");
            last = prop.clone();
        }
        if let Verdict::Differs(g, ign) = v {
            let _ = writeln!(report, "  {pos}\t{value}\t{g} 画素{}", if *ign { "・黙って無視" } else { "" });
        }
    }
    std::fs::write(out_dir.join("report.txt"), &report).map_err(|e| e.to_string())?;
    std::fs::write(out_dir.join("all.tsv"), &all).map_err(|e| e.to_string())?;
    println!("{}", report.lines().take(6).collect::<Vec<_>>().join("\n"));
    println!("{} に report.txt・all.tsv・svg/ を書いた", out_dir.display());
    Ok(())
}

/// JavaScript の文字列の字句（" と \ を escape する）。
fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}
