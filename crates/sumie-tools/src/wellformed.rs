//! `wellformed`：XML の整形式の厳しさを、roxmltree（usvg 0.48.1 が使う。usvg の設定 `allow_dtd`）と照らす
//! （requirements.md FR-IN-04・NFR-10・roadmap M1）。sumie は `sumie::fuzzing::xml_parse`（数える段と作る段）。
//!
//! `wellformed [--root corpus] [--kura-fuzz <Kura の fuzz/corpus/decode_svg>] [--mutate N] [--out local/wellformed]`
//!
//! 分け方：両方読む・両方断る・roxmltree だけ読む（sumie が断りすぎ。Kura が描くものを断ると NFR-10 に反しうる）・
//! sumie だけ読む（sumie がゆるい）・sumie が `Unsupported`（DTD の実体など。今の Kura の前段と同じ断り）・sumie が上限で
//! 止まる（`Limit`。整形式の違いでない）・sumie の不変の条件が崩れた（`Violation`。sumie の不具合）。
//! Kura のファズのコーパスは読むだけで、結果の一覧に道を書かず BLAKE3 で指す（写さない）。

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use resvg::usvg::roxmltree;
use sumie::Error;
use sumie_testkit::chacha::ChaCha;
use sumie_testkit::jsonl::Line;
use sumie_testkit::mutate::mutate;

use crate::corpus::{arg, check_args, num_arg, workspace_root};

/// 照らす端の例（このリポジトリのために書いたもの）。
fn edge_cases() -> Vec<(&'static str, String)> {
    let svg = "xmlns=\"http://www.w3.org/2000/svg\"";
    let mut v: Vec<(&'static str, String)> = vec![
        ("comment-double-dash", format!("<svg {svg}><!-- a -- b --></svg>")),
        ("comment-dash-end", format!("<svg {svg}><!-- a ---></svg>")),
        ("cdata-end-in-text", format!("<svg {svg}>a ]]> b</svg>")),
        ("control-char-text", format!("<svg {svg}>\u{1}</svg>")),
        ("control-char-attr", format!("<svg {svg} a=\"\u{1}\"/>")),
        ("char-ref-control", format!("<svg {svg}>&#1;</svg>")),
        ("char-ref-zero", format!("<svg {svg}>&#0;</svg>")),
        ("char-ref-surrogate", format!("<svg {svg}>&#xD800;</svg>")),
        ("char-ref-big", format!("<svg {svg}>&#x110000;</svg>")),
        ("decl-version-11", format!("<?xml version=\"1.1\"?><svg {svg}/>")),
        ("decl-no-version", format!("<?xml encoding=\"UTF-8\"?><svg {svg}/>")),
        ("decl-after-space", format!(" <?xml version=\"1.0\"?><svg {svg}/>")),
        ("decl-uppercase", format!("<?XML version=\"1.0\"?><svg {svg}/>")),
        ("pi-stylesheet", format!("<?xml-stylesheet href=\"a.css\"?><svg {svg}/>")),
        ("attr-no-space", format!("<svg {svg} a=\"1\"b=\"2\"/>")),
        ("attr-no-quote", format!("<svg {svg} a=1/>")),
        ("attr-lt", format!("<svg {svg} a=\"<\"/>")),
        ("attr-gt", format!("<svg {svg} a=\">\"/>")),
        ("attr-amp-bare", format!("<svg {svg} a=\"&\"/>")),
        ("text-amp-bare", format!("<svg {svg}>a & b</svg>")),
        ("undefined-entity", format!("<svg {svg}>&nope;</svg>")),
        ("duplicate-attr", format!("<svg {svg} a=\"1\" a=\"2\"/>")),
        ("duplicate-ns-attr", format!("<svg {svg} xmlns:a=\"urn:x\" xmlns:b=\"urn:x\" a:x=\"1\" b:x=\"2\"/>")),
        ("unbound-prefix-element", "<a:svg/>".to_string()),
        ("unbound-prefix-attr", format!("<svg {svg} a:x=\"1\"/>")),
        ("doctype-lowercase", format!("<!doctype svg><svg {svg}/>")),
        ("doctype-twice", format!("<!DOCTYPE svg><!DOCTYPE svg><svg {svg}/>")),
        ("doctype-after-root", format!("<svg {svg}/><!DOCTYPE svg>")),
        ("doctype-junk-in-subset", format!("<!DOCTYPE svg [ junk ]><svg {svg}/>")),
        ("doctype-attlist-default", format!("<!DOCTYPE svg [<!ATTLIST svg fill CDATA \"red\">]><svg {svg}/>")),
        ("entity-param", format!("<!DOCTYPE svg [<!ENTITY % p \"x\">]><svg {svg}/>")),
        ("entity-external", format!("<!DOCTYPE svg [<!ENTITY e SYSTEM \"x\">]><svg {svg}/>")),
        ("entity-markup", format!("<!DOCTYPE svg [<!ENTITY e \"<g/>\">]><svg {svg}>&e;</svg>")),
        ("entity-charref", format!("<!DOCTYPE svg [<!ENTITY e \"&#65;\">]><svg {svg}>&e;</svg>")),
        ("entity-in-attr", format!("<!DOCTYPE svg [<!ENTITY e \"red\">]><svg {svg} fill=\"&e;\"/>")),
        ("text-after-root", format!("<svg {svg}/>x")),
        ("two-roots", format!("<svg {svg}/><svg {svg}/>")),
        ("no-root", "<!-- only -->".to_string()),
        ("empty", String::new()),
        ("end-tag-space", format!("<svg {svg}></svg >")),
        ("end-tag-attr", format!("<svg {svg}></svg a=\"1\">")),
        ("name-digit", format!("<svg {svg}><1/></svg>")),
        ("name-colon-two", format!("<svg {svg}><a:b:c/></svg>")),
        ("name-colon-start", format!("<svg {svg}><:a/></svg>")),
        ("xmlns-empty-default", format!("<svg {svg}><g xmlns=\"\"/></svg>")),
        ("xmlns-empty-prefix", format!("<svg {svg} xmlns:p=\"\"/>")),
        ("xml-prefix-rebind", format!("<svg {svg} xmlns:xml=\"urn:x\"/>")),
        ("bom-decl", format!("\u{feff}<?xml version=\"1.0\"?><svg {svg}/>")),
        ("nbsp-in-tag", format!("<svg {svg}\u{a0}a=\"1\"/>")),
        ("tab-newline-in-tag", format!("<svg\t{svg}\na=\"1\"\r\n/>")),
        ("cdata-in-prolog", format!("<![CDATA[x]]><svg {svg}/>")),
        ("unknown-markup-decl", format!("<svg {svg}><!FOO></svg>")),
        ("pi-no-target", format!("<svg {svg}><? x?></svg>")),
        ("standalone-yes", format!("<?xml version=\"1.0\" standalone=\"yes\"?><svg {svg}/>")),
        ("encoding-latin1-ascii", format!("<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><svg {svg}/>")),
    ];
    v.extend([
        ("pi-xml-in-content", format!("<svg {svg}><?xml?></svg>")),
        ("pi-xml-tab", format!("<svg {svg}><?xml\tx?></svg>")),
        ("pi-xml-after-root", format!("<svg {svg}/><?xml\nx?>")),
        ("pi-xml-in-subset", format!("<!DOCTYPE svg [<?xml?>]><svg {svg}/>")),
        ("decl-newline", format!("<?xml\n version=\"1.0\"\n standalone=\"no\" encoding=\"UTF-8\"?><svg {svg}/>")),
        ("decl-space-in-content", format!("<svg {svg}><?xml x?></svg>")),
        ("xmlns-xmlns", format!("<svg {svg} xmlns:xmlns=\"urn:a\"/>")),
        ("xmlns-xmlns-used", format!("<svg {svg} xmlns:xmlns=\"urn:a\"><xmlns:g/></svg>")),
        ("attlist-unclosed-quote", format!("<!DOCTYPE svg [<!ATTLIST svg a CDATA \"x>]><svg {svg}/>")),
        ("attlist-gt-in-quote", format!("<!DOCTYPE svg [<!ATTLIST svg a CDATA \"x>y\">]><svg {svg}/>")),
        ("entity-cdata-end-text", format!("<!DOCTYPE svg [<!ENTITY e \"]]>\">]><svg {svg}>&e;</svg>")),
        ("entity-cdata-end-attr", format!("<!DOCTYPE svg [<!ENTITY e \"]]>\">]><svg {svg} a=\"&e;\"/>")),
    ]);
    v.push(("deep-1000", format!("<svg {svg}>{}{}</svg>", "<g>".repeat(1000), "</g>".repeat(1000))));
    v
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Both,
    Neither,
    RoxOnly,
    SumieOnly,
    SumieUnsupported,
    SumieLimit,
    SumieViolation,
}

fn judge(data: &[u8]) -> (Verdict, String, String) {
    let rox = match std::str::from_utf8(data) {
        Ok(s) => {
            let opt = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
            match roxmltree::Document::parse_with_options(s, opt) {
                Ok(_) => Ok(()),
                Err(e) => Err(e.to_string()),
            }
        }
        Err(_) => Err("UTF-8 でない".to_string()),
    };
    let out = sumie::fuzzing::xml_parse(data);
    let violation = match out.check {
        sumie::fuzzing::Check::Ok => None,
        sumie::fuzzing::Check::Violation(what) => Some(what),
    };
    let s = match out.error {
        None => Ok(()),
        Some(e) => Err(e),
    };
    let rox_text = match &rox {
        Ok(()) => "読む".to_string(),
        Err(e) => e.clone(),
    };
    let sumie_text = match (&s, violation) {
        (_, Some(what)) => format!("不変の条件が崩れた：{what}"),
        (Ok(()), None) => "読む".to_string(),
        (Err(e), None) => format!("{e:?}"),
    };
    if violation.is_some() {
        return (Verdict::SumieViolation, rox_text, sumie_text);
    }
    let v = match (&rox, &s) {
        (_, Err(Error::Limit(_))) => Verdict::SumieLimit,
        (Ok(()), Ok(())) => Verdict::Both,
        (Err(_), Err(_)) => Verdict::Neither,
        (Ok(()), Err(Error::Unsupported(_))) => Verdict::SumieUnsupported,
        (Ok(()), Err(_)) => Verdict::RoxOnly,
        (Err(_), Ok(())) => Verdict::SumieOnly,
    };
    (v, rox_text, sumie_text)
}

fn walk_svgs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_svgs(&p, out);
        } else if p.is_file() {
            out.push(p);
        }
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    check_args(args, &["--root", "--kura-fuzz", "--mutate", "--out"], &[], 0)?;
    let ws = workspace_root();
    let root = arg(args, "--root").map(PathBuf::from).unwrap_or_else(|| ws.join("corpus"));
    let out_dir = arg(args, "--out").map(PathBuf::from).unwrap_or_else(|| ws.join("local/wellformed"));
    let mutations = num_arg::<usize>(args, "--mutate", 0)?;
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    // (出どころ, 名前, バイト列)
    let mut items: Vec<(String, String, Vec<u8>)> = Vec::new();
    for (name, src) in edge_cases() {
        items.push(("端の例".into(), name.into(), src.into_bytes()));
    }
    let mut files = Vec::new();
    walk_svgs(&root, &mut files);
    files.retain(|p| p.extension().is_some_and(|x| x == "svg"));
    files.sort();
    for f in &files {
        let rel = f.strip_prefix(&root).unwrap_or(f).to_string_lossy().to_string();
        items.push(("照合の集まり".into(), rel, std::fs::read(f).map_err(|e| e.to_string())?));
    }
    if let Some(k) = arg(args, "--kura-fuzz") {
        let mut kf = Vec::new();
        walk_svgs(Path::new(k), &mut kf);
        kf.sort();
        for f in &kf {
            let bytes = std::fs::read(f).map_err(|e| e.to_string())?;
            // 道は書かない（Kura の手元のもの）
            let name = blake3::hash(&bytes).to_hex().to_string();
            items.push(("Kura のファズのコーパス".into(), name, bytes));
        }
    }
    if mutations > 0 {
        let seeds: Vec<Vec<u8>> =
            items.iter().filter(|i| i.0 == "照合の集まり").take(300).map(|i| i.2.clone()).collect();
        let dict: &[&[u8]] =
            &[b"<", b">", b"&", b";", b"--", b"]]>", b"<!DOCTYPE svg [", b"]>", b"\"", b"'", b"\x01", b"&#1;"];
        let mut rng = ChaCha::from_u64(0x5eed_0101);
        for k in 0..mutations {
            let Some(seed) = seeds.get(k % seeds.len().max(1)) else { break };
            let m = mutate(&mut rng, seed, dict, seed.len().saturating_add(64));
            items.push(("変異".into(), format!("mutate-{k}"), m));
        }
    }
    let mut counts: std::collections::BTreeMap<(String, String), usize> = Default::default();
    let mut lines = String::new();
    let mut diffs = String::new();
    for (src, name, data) in &items {
        let (v, rox, sumie) = judge(data);
        let key = format!("{v:?}");
        *counts.entry((src.clone(), key.clone())).or_default() += 1;
        let l = Line::new()
            .str("source", src.as_str())
            .str("name", name.as_str())
            .str("verdict", key.as_str())
            .str("roxmltree", rox.as_str())
            .str("sumie", sumie.as_str());
        lines.push_str(&l.to_json());
        lines.push('\n');
        if matches!(v, Verdict::RoxOnly | Verdict::SumieOnly | Verdict::SumieLimit | Verdict::SumieViolation)
            || src == "端の例"
        {
            let _ = writeln!(diffs, "| {src} | `{name}` | {key} | {rox} | {sumie} |");
        }
        // 食い違った入力は diff/ に写す（手元の調べのため。local/ の中だけ）
        if matches!(v, Verdict::RoxOnly | Verdict::SumieOnly | Verdict::SumieViolation) {
            let dir = out_dir.join("diff");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let file = name.replace(['/', '\\'], "_");
            std::fs::write(dir.join(format!("{file}.svg")), data).map_err(|e| e.to_string())?;
        }
    }
    std::fs::write(out_dir.join("result.jsonl"), lines).map_err(|e| e.to_string())?;
    let mut md =
        String::from("# XML の整形式の照らし（roxmltree と sumie）\n\n| 出どころ | 判定 | 件数 |\n|---|---|---|\n");
    for ((src, v), n) in &counts {
        let _ = writeln!(md, "| {src} | {v} | {n} |");
    }
    md.push_str("\n| 出どころ | 名前 | 判定 | roxmltree | sumie |\n|---|---|---|---|---|\n");
    md.push_str(&diffs);
    std::fs::write(out_dir.join("summary.md"), &md).map_err(|e| e.to_string())?;
    println!("{} 件 → {}", items.len(), out_dir.join("summary.md").display());
    Ok(())
}
