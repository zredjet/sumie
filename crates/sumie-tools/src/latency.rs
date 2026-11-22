//! 中止の間と手間の係数の計測（`sumie-tools latency`）。敵対的な入力を作って `sumie::parse` に通し、手間（単位）・実時間・
//! 1 単位あたりの実時間・中止の印を読む間の実時間の最大を表にする（FR-LIM-04・FR-LIM-05・NFR-05・NFR-06）。払わない長い
//! ループは、単位で見る検査では見えず、ここで「中止の印を読む間」が延びて見える（レビューで見つかった形を集めた）。

use std::sync::Mutex;
use std::time::Instant;

use crate::corpus::{arg, check_args, num_arg};

/// 中止の印を読むたびに、前に読んだときからの実時間を測る（いちばん長い間を覚える）。
struct GapMeter {
    state: Mutex<(Instant, f64)>,
}

impl sumie::Cancel for GapMeter {
    fn cancelled(&self) -> bool {
        if let Ok(mut s) = self.state.lock() {
            let now = Instant::now();
            let gap = now.duration_since(s.0).as_secs_f64() * 1000.0;
            if gap > s.1 {
                s.1 = gap;
            }
            s.0 = now;
        }
        false
    }
}

/// 名前と入力。
fn cases(scale: usize) -> Vec<(&'static str, Vec<u8>)> {
    const SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\">";
    let n = |base: usize| base.saturating_mul(scale).max(1);
    let doc = |css: &str, body: &str| format!("{SVG}<style>{css}</style>{body}</svg>").into_bytes();
    let mut out: Vec<(&'static str, Vec<u8>)> = Vec::new();
    out.push(("ふつう：.class の規則 × 要素", {
        let css: String = (0..2000).map(|i| format!(".c{i}{{fill:red}}")).collect();
        let body: String = (0..n(20_000)).map(|i| format!("<rect class=\"c{}\"/>", i % 2000)).collect();
        doc(&css, &body)
    }));
    out.push(("Illustrator 風：.stN 2000 × path", {
        let css: String = (0..2000).map(|i| format!(".st{i}{{fill:#000;stroke:#000;stroke-miterlimit:10;}}")).collect();
        let body: String =
            (0..n(50_000)).map(|i| format!("<path class=\"st{}\" d=\"M0 0h1v1z\"/>", i % 2000)).collect();
        doc(&css, &body)
    }));
    out.push((
        "長い class の名前 × 要素",
        doc(&format!(".{}{{fill:red}}", "a".repeat(n(400_000))), &"<g class=\"x\"/>".repeat(n(2000))),
    ));
    out.push(("多くの複合（a a a …）", doc(&format!("{}{{fill:red}}", vec!["a"; n(300_000)].join(" ")), "<a/>")));
    out.push((
        "当てない擬似クラスの鎖 × 要素",
        doc(&format!("{}{{fill:red}}", vec![":hover"; n(2_000)].join(" ")), &"<g/>".repeat(n(2_000))),
    ));
    out.push(("兄弟 × :first-child", doc("g:first-child{fill:red}", &"<g/>".repeat(n(200_000)))));
    out.push((
        "多くの層",
        doc(&format!("@layer {};", (0..n(100_000)).map(|i| format!("l{i}")).collect::<Vec<_>>().join(",")), ""),
    ));
    out.push((
        "多くの @namespace",
        doc(&(0..n(20_000)).map(|i| format!("@namespace p{i} url(urn:{i});")).collect::<String>(), ""),
    ));
    out.push(("文字列の改行の続き", doc(&format!("[a=\"{}x\"]{{fill:red}}", "\\\n".repeat(n(400_000))), "<g/>")));
    out.push(("長い数", doc(&format!("rect{{stroke-width:{}1px}}", "0".repeat(n(1_000_000))), "<rect/>")));
    out.push(("属性の多い根 × :lang", {
        let attrs: String = (0..n(100_000)).map(|i| format!(" a{i}=\"\"")).collect();
        let css: String = (0..100).map(|_| ":lang(en){fill:red}").collect();
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\"{attrs} lang=\"en\"><style>{css}</style>{}</svg>",
            "<g/>".repeat(1000)
        )
        .into_bytes()
    }));
    out.push(("実体の表 × 参照", {
        let ents: String = (0..n(50_000)).map(|i| format!("<!ENTITY e{i} \"\">")).collect();
        let refs: String = (0..n(100_000)).map(|i| format!("&e{};", (i * 7919) % n(50_000))).collect();
        format!("<!DOCTYPE svg [{ents}]>{SVG}<text>{refs}</text></svg>").into_bytes()
    }));
    out.push(("壊れた ATTLIST × 要素", {
        let decls: String = (0..n(20_000)).map(|_| " a CDATA \"&n;\"").collect();
        format!("<!DOCTYPE svg [<!ATTLIST g{decls}>]>{SVG}{}</svg>", "<g/>".repeat(n(5_000))).into_bytes()
    }));
    // Kura の svg.rs の試験の CSS の形（css_costs_are_bounded・css_and_reference_work_is_bounded）
    out.push((
        "Kura：隣接の結合子の鎖 g+ × 要素",
        doc(&format!("{}g{{fill:red}}", "g+".repeat(n(11_000))), &"<g/>".repeat(n(11_000))),
    ));
    out.push((
        "Kura：子孫の後戻り（61 段 × 6 つの g）",
        doc("y g g g g g g rect{fill:red}", &format!("{}<rect/>{}", "<g>".repeat(61), "</g>".repeat(61))),
    ));
    out.push(("Kura：1 つの規則の 8 万の宣言", doc(&format!("g{{{}}}", "opacity:1;".repeat(n(80_000))), "<g/>")));
    out.push((
        "Kura：style 属性の 30 万の宣言",
        format!("{SVG}<g style=\"{}\"/></svg>", "opacity:1;".repeat(n(300_000))).into_bytes(),
    ));
    out.push(("Kura：規則 12,000 × 要素 12,000", {
        let css: String = (0..12_000).map(|i| format!(".c{i}{{fill:red}}")).collect();
        let body: String = (0..n(12_000)).map(|i| format!("<g class=\"c{}\"/>", i % 12_000)).collect();
        doc(&css, &body)
    }));
    out.push(("Kura：受け継ぐ参照の規則 4,000 × 深い入れ子", {
        let css: String = (0..4_000).map(|i| format!("g{{fill:url(#p{i})}}")).collect();
        let pats: String = (0..4_000).map(|i| format!("<pattern id=\"p{i}\"/>")).collect();
        doc(
            &css,
            &format!("<defs>{pats}</defs>{}{}{}", "<g>".repeat(60), "<rect/>".repeat(n(5_000)), "</g>".repeat(60)),
        )
    }));
    // CSS の 3 回目のレビューの形
    out.push(("escape の多い @namespace の接頭辞", {
        let p = "\\70".repeat(n(2_000_000));
        doc(&format!("<![CDATA[@namespace {p}q url(urn:x); {p}q|rect{{fill:red}}]]>"), "<rect/>")
    }));
    out.push((
        "当たる選択子の多い規則 × 要素",
        doc(&format!("{}{{fill:red}}", vec!["rect"; n(200_000)].join(",")), &"<rect/>".repeat(20)),
    ));
    // CSS の 4 回目のレビューの形
    out.push(("font のまとめ × 要素", {
        let css: String = (0..100).map(|_| format!("rect{{{}}}", "font:1px a;".repeat(20))).collect();
        doc(&css, &"<rect/>".repeat(n(5_000)))
    }));
    out.push((
        "偽の条件の長い複合 × 要素",
        doc(&format!("@media print{{{}{{fill:red}}}}", vec!["*"; 50_000].join(" ")), &"<g/>".repeat(n(5_000))),
    ));
    out.push(("@namespace と壊れた規則の交互", {
        let css: String = (0..n(10_000)).map(|i| format!("@namespace p{i} url(urn:{i});!{{}}")).collect();
        doc(&css, "<g/>")
    }));
    out.push((
        "class の語の重なり",
        doc(&".a{fill:red}".repeat(1_000), &format!("<rect class=\"{}\"/>", vec!["a"; n(100_000)].join(" "))),
    ));
    out.push((
        "escape の擬似クラスの名前",
        doc(&format!("rect{}{{fill:red}}", ":\\000066ocus-visible".repeat(n(20_000))), "<rect/>"),
    ));
    out.push((
        "@media not の問いの並び",
        doc(&format!("@media {} {{rect{{fill:red}}}}", vec!["not a"; n(20_000)].join(", ")), "<rect/>"),
    ));
    // 文書の段の 1 回目のレビューの形
    out.push((
        "g の下の多くの文字の節",
        format!("{SVG}<g>{}</g></svg>", "a<![CDATA[b]]>".repeat(n(700_000))).into_bytes(),
    ));
    out.push((
        "1 つの要素の多くの filter の url",
        format!("{SVG}<g id=\"a\"><rect style=\"filter:{}\"/></g></svg>", "url(#a) ".repeat(n(1_500_000))).into_bytes(),
    ));
    out.push((
        "子の辺で戻る輪 × 12,000",
        format!(
            "{SVG}<use href=\"#c\"/><g id=\"top\"><g id=\"c\">{}</g></g></svg>",
            "<use href=\"#top\"/>".repeat(n(12_000))
        )
        .into_bytes(),
    ));
    // 文書の段の 2 回目のレビューの形
    out.push(("巻き戻しの多い輪（K = D）", {
        let k = n(8_000);
        let uses: String = (0..k).map(|i| format!("<use href=\"#c{i}\"/>")).collect();
        let inner: String = (0..k).map(|i| format!("<use id=\"c{i}\" href=\"#Q\"/>")).collect();
        format!("{SVG}{uses}<g id=\"Q\">{}<g id=\"P\">{inner}</g>{}</g></svg>", "<g>".repeat(1000), "</g>".repeat(1000))
            .into_bytes()
    }));
    out.push(("倍々の use 100 段", {
        let mut body = String::from("<defs><rect id=\"l0\"/>");
        for k in 1..=100 {
            body.push_str(&format!("<g id=\"l{k}\"><use href=\"#l{p}\"/><use href=\"#l{p}\"/></g>", p = k - 1));
        }
        format!("{SVG}{body}</defs><use href=\"#l100\"/></svg>").into_bytes()
    }));
    out.push(("filter の inherit × 長い並び", {
        let filters: String = (0..40_000).map(|i| format!("<filter id=\"f{i}\"/>")).collect();
        let urls: String = (0..40_000).map(|i| format!("url(#f{i}) ")).collect();
        doc(
            "g g{filter:inherit}",
            &format!("{filters}<g style=\"filter:{urls}\">{}{}</g>", "<g>".repeat(1000), "</g>".repeat(1000)),
        )
    }));
    out.push((
        "規則の長い filter × 要素",
        doc(
            &format!("rect{{filter:{}}}", "url(#f) ".repeat(10_000)),
            &format!("<filter id=\"f\"/>{}", "<rect/>".repeat(n(10_000))),
        ),
    ));
    // 文書の段の 3 回目のレビューの形
    out.push(("輪の鎖 × 多くの節", {
        let masks: String = (1..=1000)
            .map(|i| format!("<mask id=\"Y{i}\"><rect fill=\"url(#X)\" mask=\"url(#Y{})\"/></mask>", i - 1))
            .collect();
        format!(
            "{SVG}<pattern id=\"X\"><mask id=\"Y0\"><rect/></mask>{masks}</pattern><rect mask=\"url(#Y1000)\"/>{}</svg>",
            "<g/>".repeat(n(400_000))
        )
        .into_bytes()
    }));
    // 文書の段の 4 回目のレビューの形
    out.push((
        "受け継ぐ参照の長い外の URL × 要素",
        doc(&format!("path{{marker:url(x.svg#{})}}", "a".repeat(20_000)), &"<path d=\"M0 0L1 1\"/>".repeat(n(20_000))),
    ));
    out.push(("規則の長い #id の塗り × 要素", {
        let id = "a".repeat(20_000);
        doc(
            &format!("rect{{fill:url(#{id})}}"),
            &format!("<linearGradient id=\"{id}\"/>{}", "<rect/>".repeat(n(20_000))),
        )
    }));
    out.push(("use の中身の mask の inherit の爆弾", {
        let rects = "<rect style=\"mask:inherit\"/>".repeat(10);
        let mut body = format!("<defs><mask id=\"k0\"><rect/></mask><g id=\"c\" style=\"mask:inherit\">{rects}</g>");
        for i in 1..=8 {
            body.push_str(&format!("<mask id=\"k{i}\"><use href=\"#c\" mask=\"url(#k{})\"/></mask>", i - 1));
        }
        format!("{SVG}{body}</defs><rect mask=\"url(#k8)\"/></svg>").into_bytes()
    }));
    out.push(("深い入れ子", format!("{SVG}{}{}</svg>", "<g>".repeat(1000), "</g>".repeat(1000)).into_bytes()));
    out.push(("参照の多い文書（use の鎖）", {
        let mut body = String::from("<defs><rect id=\"r0\"/>");
        for k in 1..n(2_000) {
            body.push_str(&format!("<g id=\"r{k}\"><use href=\"#r{}\"/></g>", k - 1));
        }
        body.push_str("</defs>");
        format!("{SVG}{body}</svg>").into_bytes()
    }));
    out
}

pub fn run(args: &[String]) -> Result<(), String> {
    check_args(args, &["--scale", "--filter"], &[], 0)?;
    let scale = num_arg::<usize>(args, "--scale", 1)?;
    let filter = arg(args, "--filter");
    let opts = sumie::ParseOptions::DEFAULT;
    println!("| 入力 | バイト | 手間（単位） | 実時間（ms） | ns／単位 | 中止を読む間の最大（ms） | 結果 |");
    println!("|---|---|---|---|---|---|---|");
    for (name, data) in cases(scale) {
        if filter.is_some_and(|f| !name.contains(f)) {
            continue;
        }
        let est = sumie::estimate_parse(&data, &opts, &sumie::Never);
        let meter = GapMeter { state: Mutex::new((Instant::now(), 0.0)) };
        let t0 = Instant::now();
        if let Ok(mut s) = meter.state.lock() {
            s.0 = t0;
        }
        let r = sumie::parse(&data, &opts, &meter);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let gap = meter.state.lock().map(|s| s.1).unwrap_or(0.0);
        let (work, result) = match (&est, &r) {
            (Ok(e), Ok(_)) => (e.work, "読む".to_string()),
            (_, Err(e)) => (0, format!("{e:?}")),
            (Err(e), Ok(_)) => (0, format!("見積もり：{e:?}")),
        };
        let per = if work > 0 { ms * 1e6 / work as f64 } else { 0.0 };
        println!(
            "| {name} | {} | {work} | {ms:.1} | {per:.2} | {gap:.3} | {} |",
            data.len(),
            result.chars().take(60).collect::<String>()
        );
    }
    Ok(())
}
