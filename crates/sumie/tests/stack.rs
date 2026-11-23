//! 小さいスタック（128 KiB）の試験（NFR-03）。debug と release の両方で回す（CI は `cargo test --release` も回す）。
//! スタックがあふれたらプロセスごと落ちるので、落ちずに終われば通る。M1 は 10 万段の入れ子・参照の鎖・セレクタの
//! 結合子・括弧と規則の入れ子を、読む段の全体と木を作る段の見積もりに通す（Kura の攻撃の例は tests/attacks.rs も
//! 小さなスタックで回す）。木を作る・描く段ができたら、同じ例を最後まで通す。

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects
)]

use sumie::Limits;
use sumie::fuzzing::{Check, mem_ops, xml_parse_with};
use sumie_testkit::stack::on_small_stack;

#[test]
fn mem_ops_on_a_small_stack() {
    // 大きな上限で、伸ばす・並べ替える（8.5 万個。並べ替えは再帰しない）
    let mut ops = vec![23u8, 19];
    for i in 0..5000u32 {
        ops.extend_from_slice(&[1, 16, 0, (i % 251) as u8]);
    }
    ops.extend_from_slice(&[7, 0, 7, 3, 11, 0, 11, 7, 7, 1]);
    let out = on_small_stack(move || mem_ops(&ops));
    assert_eq!(out.check, Check::Ok);
    assert!(!out.limited, "上限で止まった：{out:?}");
    assert!(out.work > 100_000);
}

/// 10 万段の入れ子の XML を、上限を広げて 128 KiB のスタックで読む（歩きは再帰しない）。
#[test]
fn xml_deep_nesting_on_a_small_stack() {
    let n = 100_000;
    let mut src = String::from("<svg xmlns=\"http://www.w3.org/2000/svg\">");
    for _ in 0..n {
        src.push_str("<g a=\"1\">");
    }
    for _ in 0..n {
        src.push_str("</g>");
    }
    src.push_str("</svg>");
    let limits = Limits { depth: 200_000, nodes: 400_000, ..Limits::DEFAULT };
    let out = on_small_stack(move || xml_parse_with(src.as_bytes(), &limits));
    assert_eq!(out.check, Check::Ok, "{out:?}");
    assert_eq!(out.nodes, Some(n + 1), "{out:?}");
}

/// 公開の口（estimate_parse・parse）でも、深い入れ子を小さなスタックで読める（読む段の全体の段が増えても見張る）。
#[test]
fn parse_deep_nesting_on_a_small_stack() {
    let n = 100_000;
    let src = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><style>g {{ fill: red }}</style>{}{}</svg>",
        "<g a=\"1\">".repeat(n),
        "</g>".repeat(n)
    );
    let opts = sumie::ParseOptions {
        limits: Limits { depth: 200_000, nodes: 400_000, ..Limits::DEFAULT },
        ..sumie::ParseOptions::DEFAULT
    };
    let (e, p) = on_small_stack(move || {
        let e = sumie::estimate_parse(src.as_bytes(), &opts, &sumie::Never).map(|_| ());
        let p = sumie::parse(src.as_bytes(), &opts, &sumie::Never).map(|_| ());
        (e, p)
    });
    assert_eq!((e, p), (Ok(()), Ok(())));
}

/// estimate_parse・parse と、木を作る段から描く段まで（estimate_build・build・計画・4 × 4 の範囲を描く）の結果（読めなければ
/// 後ろは None）。
type Stages = (Result<(), sumie::Error>, Result<(), sumie::Error>, Option<Result<(), sumie::Error>>);

/// 読む段の全体（estimate_parse・parse）と、木を作る段・計画・描く段を 128 KiB のスタックで通し、結果を返す（単位 3 の
/// 2 回目のレビュー：木を作る段そのものも小さなスタックで通す）。
fn parse_small(src: String, limits: Limits) -> Stages {
    let opts = sumie::ParseOptions { limits, ..sumie::ParseOptions::DEFAULT };
    on_small_stack(move || {
        let e = sumie::estimate_parse(src.as_bytes(), &opts, &sumie::Never).map(|_| ());
        let p = sumie::parse(src.as_bytes(), &opts, &sumie::Never);
        let b = p.as_ref().ok().map(|d| {
            d.estimate_build(&sumie::Never)?;
            let t = d.build(&sumie::Never)?;
            let plan = t.plan(&sumie::RenderOptions::DEFAULT, &sumie::Never)?;
            let size = plan.pixel_size();
            let rect = sumie::PixelRect { x: 0, y: 0, width: size.width.min(4), height: size.height.min(4) };
            let mut px = vec![0u8; rect.width as usize * rect.height as usize * 4];
            let mut out = sumie::PixmapMut::new(&mut px, rect.width, rect.height, rect.width as usize * 4)?;
            plan.render_region(rect, &mut out, &sumie::Never)
        });
        (e, p.map(|_| ()), b)
    })
}

/// 参照の鎖（use・pattern の href・gradient の href・受け継ぐ塗り）が 10 万段でも、輪を探す・展開の数を数える歩きは
/// 再帰しない。
#[test]
fn reference_chains_on_a_small_stack() {
    let n = 100_000;
    let x = "xmlns=\"http://www.w3.org/2000/svg\" width=\"4\" height=\"4\"";
    let mut uses = format!("<svg {x}><defs><rect id=\"u0\"/>");
    let mut pats = format!("<svg {x}><defs><pattern id=\"p0\"/>");
    let mut grads = format!("<svg {x}><defs><linearGradient id=\"g0\"/>");
    for i in 1..n {
        uses.push_str(&format!("<g id=\"u{i}\"><use href=\"#u{}\"/></g>", i - 1));
        pats.push_str(&format!("<pattern id=\"p{i}\"><rect fill=\"url(#p{})\"/></pattern>", i - 1));
        grads.push_str(&format!("<linearGradient id=\"g{i}\" href=\"#g{}\"/>", i - 1));
    }
    uses.push_str(&format!("</defs><use href=\"#u{}\"/></svg>", n - 1));
    pats.push_str(&format!("</defs><rect fill=\"url(#p{})\"/></svg>", n - 1));
    grads.push_str(&format!("</defs><rect fill=\"url(#g{})\"/></svg>", n - 1));
    // defs の中の鎖は描く参照の分だけ数えるので、展開の数は 2n ほどで上限の内
    for src in [uses, pats, grads] {
        assert_eq!(parse_small(src, Limits::DEFAULT), (Ok(()), Ok(()), Some(Ok(()))));
    }
    // 輪の鎖（最後が最初を指す）
    let mut ring = format!("<svg {x}><defs>");
    for i in 0..n {
        ring.push_str(&format!("<g id=\"r{i}\"><use href=\"#r{}\"/></g>", (i + 1) % n));
    }
    ring.push_str("</defs><use href=\"#r0\"/></svg>");
    assert_eq!(parse_small(ring, Limits::DEFAULT), (Ok(()), Ok(()), Some(Ok(()))));
}

/// 長いセレクタ（結合子 10 万・`:is()`・`:not()` の入れ子）・入れ子の括弧・入れ子の規則を、小さなスタックで読む。
#[test]
fn long_selectors_on_a_small_stack() {
    let n = 100_000;
    let doc = |css: String| {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"4\" height=\"4\"><style>{css}</style>{}<rect/>{}</svg>",
            "<g>".repeat(10),
            "</g>".repeat(10)
        )
    };
    let cases = [
        format!("{}rect{{fill:red}}", "g ".repeat(n)),
        format!("{}rect{{fill:red}}", "g>".repeat(n)),
        format!("{}rect{{fill:red}}", "g~".repeat(n)),
        format!("{}rect{}{{fill:red}}", ":is(".repeat(10_000), ")".repeat(10_000)),
        format!("{}rect{}{{fill:red}}", ":not(".repeat(10_000), ")".repeat(10_000)),
        format!("rect{{fill:{}red{}}}", "(".repeat(n), ")".repeat(n)),
        format!("rect{{fill:{}red}}", "[".repeat(n)),
        format!("{}rect{{fill:red}}{}", "g{".repeat(n), "}".repeat(n)),
        format!("@media {}screen{} {{rect{{fill:red}}}}", "(".repeat(n), ")".repeat(n)),
        format!("@supports {}(fill:red){} {{rect{{fill:red}}}}", "not (".repeat(10_000), ")".repeat(10_000)),
    ];
    for css in cases {
        let head: String = css.chars().take(40).collect();
        let (e, p, _) = parse_small(doc(css), Limits::DEFAULT);
        // 読むか断る。断るなら estimate_parse と parse が同じ誤り
        match (&e, &p) {
            (Ok(()), Ok(())) => {}
            (Err(a), Err(b)) => assert_eq!(a, b, "{head}"),
            _ => panic!("{head}：{e:?} {p:?}"),
        }
    }
}

/// 実体の参照の鎖（実体の中の参照は断るので、鎖は 1 段だけ）と、深い入れ子の属性の値の参照。
#[test]
fn entity_references_on_a_small_stack() {
    let ents: String = (0..10_000).map(|i| format!("<!ENTITY e{i} \"v{i}\">")).collect();
    let refs: String = (0..10_000).map(|i| format!("&e{i};")).collect();
    let src = format!(
        "<!DOCTYPE svg [{ents}]><svg xmlns=\"http://www.w3.org/2000/svg\" width=\"4\" height=\"4\"><desc>{refs}</desc>{}<rect class=\"{refs}\"/>{}</svg>",
        "<g>".repeat(1000),
        "</g>".repeat(1000)
    );
    let (e, p, _) = parse_small(src, Limits::DEFAULT);
    assert_eq!((e, p), (Ok(()), Ok(())));
}

/// 計算した値のたどり（`doc::walk`）も 10 万段の入れ子を 128 KiB のスタックで回る（文書の 1 回目のレビュー）。
#[test]
fn computed_walk_deep_nesting_on_a_small_stack() {
    let n = 100_000;
    let src = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"><style>g {{ fill: red }}</style>{}<rect/>{}</svg>",
        "<g transform=\"translate(1 0)\" stroke-width=\"10%\">".repeat(n),
        "</g>".repeat(n)
    );
    let opts = sumie::ParseOptions {
        limits: Limits { depth: 200_000, nodes: 400_000, ..Limits::DEFAULT },
        ..sumie::ParseOptions::DEFAULT
    };
    let count = on_small_stack(move || {
        let d = sumie::parse(src.as_bytes(), &opts, &sumie::Never).unwrap();
        let mut count = 0usize;
        sumie::fuzzing::computed_values(&d, &mut |_| count += 1).unwrap();
        count
    });
    assert_eq!(count, n + 3);
}

/// 読む段から全体を描くまでを 128 KiB のスタックで通し、各段の結果と描いた画素に色が付いたか（不透明の画素の数）を返す。
fn render_small(src: String, limits: Limits) -> Result<usize, sumie::Error> {
    let opts = sumie::ParseOptions { limits, ..sumie::ParseOptions::DEFAULT };
    on_small_stack(move || {
        sumie::estimate_parse(src.as_bytes(), &opts, &sumie::Never)?;
        let d = sumie::parse(src.as_bytes(), &opts, &sumie::Never)?;
        d.estimate_build(&sumie::Never)?;
        let t = d.build(&sumie::Never)?;
        let plan = t.plan(&sumie::RenderOptions::DEFAULT, &sumie::Never)?;
        let size = plan.pixel_size();
        let mut px = vec![0u8; size.width as usize * size.height as usize * 4];
        let mut out = sumie::PixmapMut::new(&mut px, size.width, size.height, size.width as usize * 4)?;
        plan.render(&mut out, &sumie::Never)?;
        let half = sumie::PixelRect { x: 0, y: 0, width: size.width / 2, height: size.height };
        let mut part = vec![0u8; half.width as usize * half.height as usize * 4];
        let mut out = sumie::PixmapMut::new(&mut part, half.width, half.height, half.width as usize * 4)?;
        plan.render_region(half, &mut out, &sumie::Never)?;
        Ok(px.as_chunks::<4>().0.iter().filter(|p| p[3] == 255).count())
    })
}

/// 描く段の例（M2）：太い破線の長いパス（2 次曲線・弧・round の継ぎ目と端）と、1 万段の入れ子の g の中の破線の角の丸い
/// 矩形を、128 KiB のスタックで最後まで描く（平坦化・線の外形・破線・ラスタライズ・範囲の描画は再帰しない。単位 4 の
/// レビュー）。
#[test]
fn strokes_and_dashes_on_a_small_stack() {
    let x = "xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\"";
    // 行って戻る曲線と弧（5000 組）を少しずつ下へずらす
    let mut d = String::from("M20 5");
    for i in 0..5000 {
        let s = if i % 2 == 0 { 1 } else { -1 };
        d.push_str(&format!(" q 10 {} 20 0 a 12 8 {} 0 1 -20 0.01", 6 * s, i % 90));
    }
    let path = format!(
        "<svg {x}><path d=\"{d}\" fill=\"none\" stroke=\"black\" stroke-width=\"3\" stroke-dasharray=\"1 0.5\" \
         stroke-linejoin=\"round\" stroke-linecap=\"round\"/></svg>"
    );
    let n = 10_000;
    let nested = format!(
        "<svg {x}>{}<rect x=\"2\" y=\"2\" width=\"40\" height=\"30\" rx=\"5\" fill=\"green\" stroke=\"navy\" \
         stroke-width=\"2\" stroke-dasharray=\"3 1\" stroke-linejoin=\"miter-clip\"/>{}</svg>",
        "<g transform=\"rotate(0.001)\" stroke-dashoffset=\"1\">".repeat(n),
        "</g>".repeat(n)
    );
    let deep = Limits { depth: 20_000, ..Limits::DEFAULT };
    for (name, src) in [("破線の長いパス", path), ("入れ子の中の破線の矩形", nested)] {
        let painted = render_small(src, deep).unwrap_or_else(|e| panic!("{name}：{e:?}"));
        assert!(painted > 50, "{name}：描いた不透明の画素が {painted}");
    }
}
