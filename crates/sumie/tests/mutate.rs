//! 種つきの変異の試験（CLAUDE.md・architecture.md §9）。種と回数を固定し、`sumie-testkit` の ChaCha8 で作った入力を
//! ファズの入り口（`sumie::fuzzing`）に通して、不変の条件・panic しないこと・ヒープの山 ≤ 数えた山を確かめる。
//! 段ごとの対象：M0 の mem・math、M1 の xml・dtd・css・values・document（読む段の全体）、M2 の path・stroke・tree（木を作る
//! 段）・render（描く段まで）。

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects
)]

use std::sync::atomic::{AtomicU32, Ordering};

use sumie::Cancel;
use sumie::fuzzing::{
    CSS_HEAD, CSS_TAIL, Check, StageOutcome, build_ops, css_parse, document_parse, document_parse_cancel, dtd_parse,
    math_ops, mem_ops, mem_ops_with, path_ops, render_ops, stroke_ops, values_parse, xml_parse,
};
use sumie_testkit::alloc::{assert_heap_within, measure};
use sumie_testkit::mutate::{cases, value_cases};

/// mem_ops の種：上限の 2 バイトと、(操作, 引数) の並び。
/// fuzz/seeds/mem_ops と同じもの（fuzz/README.md）。
const MEM_SEEDS: &[&[u8]] = &[
    &[20, 19, 0, 1, 0, 2, 0, 3, 1, 9, 7, 0],
    &[23, 19, 2, 200, 7, 3, 11, 0, 11, 5, 9, 3],
    &[4, 6, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1],
    &[23, 19, 5, 1, 5, 1, 5, 2, 6, 3, 6, 3, 10, 3, 9, 1, 8, 0, 4, 2, 3, 0],
    // 大きく伸ばしてから並べ替え・差し込み（数えた山が数十 KB になる）
    &[23, 19, 1, 16, 1, 16, 1, 16, 1, 16, 2, 255, 1, 16, 2, 250, 7, 2, 11, 200, 11, 3, 7, 5, 2, 10, 7, 0],
];
const MEM_DICT: &[&[u8]] = &[&[0, 255], &[1, 16], &[2, 255], &[7, 1], &[11, 0], &[5, 7], &[6, 31], &[4, 3]];

#[test]
fn mem_ops_survive_seeded_mutations() {
    let mut limited = 0usize;
    for (n, case) in cases(0x5eed_0001, 4000, MEM_SEEDS, MEM_DICT, 1024).iter().enumerate() {
        let (out, heap) = measure(|| mem_ops(case));
        assert_eq!(out.check, Check::Ok, "{n} 件目 {case:?}");
        // mem の入れ物は頼んだ容量 × size_of だけを確保する（数えない確保がない）
        assert_heap_within(&heap, out.counted_peak, 0, &format!("{n} 件目"));
        assert_eq!(heap.current, 0, "{n} 件目：返していない確保");
        // 山でない時点の数えない確保も見つける：ヒープの確保の合計・回数 ≤ 数えた合計・回数
        // 数える量は頼んだ容量 × size_of で、std の確保と同じ（漏れも払いすぎもない）
        assert_eq!(
            heap.total, out.charged_total,
            "{n} 件目：確保の合計 {} と数えた {} が違う",
            heap.total, out.charged_total
        );
        assert!(heap.count <= out.charges, "{n} 件目：確保の回数 {} > 数えた {}", heap.count, out.charges);
        limited += usize::from(out.limited);
    }
    // 上限で止まる件も、止まらない件もある（試験が両方の道を通る）
    assert!(limited > 100 && limited < 3900, "上限で止まった件 {limited}");
}

/// fuzz/seeds/mem_ops の種が MEM_SEEDS と同じ（ずれない）。
#[test]
fn fuzz_seeds_match_mem_seeds() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/mem_ops");
    let mut files: Vec<Vec<u8>> =
        std::fs::read_dir(dir).unwrap().flatten().map(|e| std::fs::read(e.path()).unwrap()).collect();
    files.sort();
    let mut seeds: Vec<Vec<u8>> = MEM_SEEDS.iter().map(|s| s.to_vec()).collect();
    seeds.sort();
    assert_eq!(files, seeds, "fuzz/seeds/mem_ops と tests/mutate.rs の MEM_SEEDS を合わせる");
}

/// 決まった回数だけ読んだら立つ中止の印。
struct AfterCalls {
    left: AtomicU32,
}

impl Cancel for AfterCalls {
    fn cancelled(&self) -> bool {
        self.left.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1)).is_err()
    }
}

/// 途中で中止しても、不変の条件と「数えた量を返す」が崩れない。中止は印が立った最初の確かめで返る。
#[test]
fn mem_ops_survive_cancellation() {
    let big = MEM_SEEDS[4];
    let mut cancelled = 0usize;
    for (n, case) in cases(0x5eed_0003, 600, &[big], MEM_DICT, 2048).iter().enumerate() {
        for calls in [0u32, 1, 3] {
            let c = AfterCalls { left: AtomicU32::new(calls) };
            let (out, heap) = measure(|| mem_ops_with(case, &c));
            assert_eq!(out.check, Check::Ok, "{n} 件目（{calls} 回で中止）");
            assert_eq!(heap.current, 0);
            if out.cancelled {
                cancelled += 1;
                // 印は CANCEL_INTERVAL（2^16）ごとに読むので、calls + 1 回目の確かめまでに止まる
                assert!(out.work < (u64::from(calls) + 2) << 16, "{n} 件目：{} 単位まで回った", out.work);
            }
        }
    }
    assert!(cancelled > 0, "中止の道を通らなかった");
}

/// fuzz/seeds/math_ops と同じもの。
const MATH_SEEDS: &[&[u8]] = &[
    &[0u8; 48],
    &[0xff; 48],
    &[0, 0, 0, 0, 0, 0, 0xf0, 0x7f, 0, 0, 0, 0, 0, 0, 0xf8, 0x7f, 1, 0, 0, 0, 0, 0, 0, 0x80],
    &[0x18, 0x2d, 0x44, 0x54, 0xfb, 0x21, 0x09, 0x40, 0, 0, 0, 0, 0, 0, 0xe0, 0x3f, 0, 0, 0, 0, 0, 0, 0, 0],
];

/// fuzz/seeds/math_ops の種が MATH_SEEDS と同じ（ずれない）。
#[test]
fn fuzz_seeds_match_math_seeds() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/math_ops");
    let mut files: Vec<Vec<u8>> =
        std::fs::read_dir(dir).unwrap().flatten().map(|e| std::fs::read(e.path()).unwrap()).collect();
    files.sort();
    let mut seeds: Vec<Vec<u8>> = MATH_SEEDS.iter().map(|s| s.to_vec()).collect();
    seeds.sort();
    assert_eq!(files, seeds, "fuzz/seeds/math_ops と tests/mutate.rs の MATH_SEEDS を合わせる");
}

#[test]
fn math_ops_survive_seeded_mutations() {
    let seeds = MATH_SEEDS;
    let dict: &[&[u8]] = &[&[0, 0, 0, 0, 0, 0, 0xf0, 0x7f], &[0, 0, 0, 0, 0, 0, 0, 0x80], &[1, 0, 0, 0, 0, 0, 0, 0]];
    for (n, case) in cases(0x5eed_0002, 4000, seeds, dict, 240).iter().enumerate() {
        assert_eq!(math_ops(case), Check::Ok, "{n} 件目 {case:?}");
    }
}

/// fuzz/seeds/<対象> の種を読む（道の名前の順）。
fn fuzz_seeds(target: &str) -> Vec<Vec<u8>> {
    named_fuzz_seeds(target).into_iter().map(|(_, b)| b).collect()
}

/// fuzz/seeds/<対象> の種を、ファイルの名前つきで読む（道の名前の順）。
fn named_fuzz_seeds(target: &str) -> Vec<(String, Vec<u8>)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds").join(target);
    let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).collect();
    files.sort();
    files.iter().map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read(p).unwrap())).collect()
}

/// XML の目印の字句（変異で差し込む）。
const XML_DICT: &[&[u8]] = &[
    b"<",
    b">",
    b"/>",
    b"</",
    b"=",
    b"\"",
    b"'",
    b"&",
    b";",
    b"&amp;",
    b"&#x41;",
    b"&#9;",
    b"<!--",
    b"-->",
    b"<![CDATA[",
    b"]]>",
    b"<?",
    b"?>",
    b"<!DOCTYPE svg [",
    b"]>",
    b"<!ENTITY a \"x\">",
    b"&a;",
    b"%",
    b"xmlns:p=\"urn:p\"",
    b"p:",
    b"xml:space=\"preserve\"",
    b"\r\n",
    b"\t",
    "鞍".as_bytes(),
    b"\xff",
];

#[test]
fn xml_survives_seeded_mutations() {
    let seeds = fuzz_seeds("xml");
    let refs: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    let (mut ok, mut err) = (0usize, 0usize);
    for (n, case) in cases(0x5eed_0004, 3000, &refs, XML_DICT, 4096).iter().enumerate() {
        let (out, heap) = measure(|| xml_parse(case));
        assert_eq!(out.check, Check::Ok, "{n} 件目 {:?}", String::from_utf8_lossy(case));
        // ヒープの山 ≤ 数えた山（数える段と作る段の大きい方）
        assert_heap_within(&heap, out.count_peak.max(out.build_peak), 0, &format!("{n} 件目"));
        assert_eq!(heap.current, 0, "{n} 件目：返していない確保");
        if out.error.is_some() { err += 1 } else { ok += 1 }
    }
    // 読める件も読めない件もある（試験が両方の道を通る）
    assert!(ok > 30 && err > 100, "読めた {ok}・読めない {err}");
}

/// DTD の目印の字句（XML の目印に足す）。
const DTD_DICT: &[&[u8]] = &[
    b"<!ATTLIST svg ",
    b"<!ATTLIST g a ",
    b"<!ENTITY a \"x\">",
    b"<!ENTITY b \"&a;&a;\">",
    b"<!ELEMENT svg ANY>",
    b" CDATA ",
    b" NMTOKENS ",
    b" ID ",
    b" (a|b) ",
    b"#IMPLIED",
    b"#REQUIRED",
    b"#FIXED ",
    b"\"&a;\"",
    b"&#32;",
    b"xmlns",
    b"b:xmlns",
    b"xml:space",
    b">",
    b"\"",
];

#[test]
fn dtd_survives_seeded_mutations() {
    let seeds = fuzz_seeds("dtd");
    let refs: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    let dict: Vec<&[u8]> = XML_DICT.iter().chain(DTD_DICT).copied().collect();
    let (mut ok, mut err) = (0usize, 0usize);
    for (n, case) in cases(0x5eed_0005, 3000, &refs, &dict, 1024).iter().enumerate() {
        let (out, heap) = measure(|| dtd_parse(case));
        assert_eq!(out.check, Check::Ok, "{n} 件目 {:?}", String::from_utf8_lossy(case));
        assert_heap_within(&heap, out.count_peak.max(out.build_peak).saturating_add(4096), 0, &format!("{n} 件目"));
        assert_eq!(heap.current, 0, "{n} 件目：返していない確保");
        if out.error.is_some() { err += 1 } else { ok += 1 }
    }
    // 読める件も読めない件もある（試験が両方の道を通る）
    assert!(ok > 30 && err > 100, "読めた {ok}・読めない {err}");
}

#[test]
fn document_survives_seeded_mutations() {
    let seeds = fuzz_seeds("document");
    let refs: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    let (mut ok, mut err) = (0usize, 0usize);
    // 文書の構造の目印（use・pattern・switch・条件・入れ子の svg・変形。文書の 1 回目のレビュー）も混ぜる
    let dict: Vec<&[u8]> = XML_DICT.iter().chain(DOC_DICT).copied().collect();
    for (n, case) in cases(0x5eed_0006, 2000, &refs, &dict, 4096).iter().enumerate() {
        let (out, heap) = measure(|| document_parse(case));
        assert_eq!(out.check, Check::Ok, "{n} 件目 {:?}", String::from_utf8_lossy(case));
        let peak = out.peak;
        assert_heap_within(&heap, peak, 0, &format!("{n} 件目"));
        assert_eq!(heap.current, 0, "{n} 件目：返していない確保");
        if out.error.is_some() { err += 1 } else { ok += 1 }
    }
    assert!(ok > 30 && err > 100, "読めた {ok}・読めない {err}");
}

/// 文書の構造の目印（変異で差し込む）。
const DOC_DICT: &[&[u8]] = &[
    b"<use href=\"#r\"/>",
    b"<use xlink:href=\"#g\" x=\"10%\"/>",
    b" fill=\"url(#p)\"",
    b" stroke=\"url(#lg) none\"",
    b" mask=\"inherit\"",
    b" clip-path=\"url(#m)\"",
    b" filter=\"url(#f)\"",
    b"<pattern id=\"p\">",
    b"</pattern>",
    b"<switch>",
    b"</switch>",
    b" systemLanguage=\"en\"",
    b" requiredExtensions=\"\"",
    b"<svg x=\"1\" viewBox=\"0 0 1 1\">",
    b" transform=\"rotate(30 1 2)\"",
    b" style=\"transform:scale(2);transform-box:fill-box\"",
    b" display=\"none\"",
    b" id=\"r\"",
    b" id=\"g\"",
    b"<text>",
    b"<tspan>",
];

/// CSS の目印の字句（変異で差し込む）。
const CSS_DICT: &[&[u8]] = &[
    b"{",
    b"}",
    b"(",
    b")",
    b"[",
    b"]",
    b";",
    b":",
    b",",
    b"!important",
    b"/*",
    b"*/",
    b"\\",
    b"\\61 ",
    b"\"",
    b"'",
    b"url(",
    b"@media (min-width: 10px) {",
    b"@supports (fill: red) {",
    b"@layer a",
    b"@layer a.b, c;",
    b"@namespace x url(urn:x);",
    b"@import url(x.css);",
    b"@font-face { src: url(f) }",
    b":is(",
    b":not(",
    b":where(",
    b":has(",
    b":nth-child(2n+1 of ",
    b"::before",
    b"x|",
    b"*|",
    b"|",
    b"#r",
    b".c",
    b"&",
    b" > ",
    b" + ",
    b" ~ ",
    b"1e999",
    b"-.5e-3",
    b"rgb(1 2 3 / 50%)",
    b"var(--x)",
    b"calc(1px + 2%)",
    "鞍".as_bytes(),
];

#[test]
fn css_survives_seeded_mutations() {
    let seeds = fuzz_seeds("css");
    let refs: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    let (mut ok, mut err) = (0usize, 0usize);
    for (n, case) in cases(0x5eed_0007, 2000, &refs, CSS_DICT, 4096).iter().enumerate() {
        // 文書に入れるのは試験の側で（入れ物の確保を量に入れない）。css_parse と同じ文書
        let doc = [CSS_HEAD, case.as_slice(), CSS_TAIL].concat();
        assert_eq!(css_parse(case).check, document_parse(&doc).check);
        let (out, heap) = measure(|| document_parse(&doc));
        assert_eq!(out.check, Check::Ok, "{n} 件目 {:?}", String::from_utf8_lossy(case));
        assert_heap_within(&heap, out.peak, 0, &format!("{n} 件目"));
        assert_eq!(heap.current, 0, "{n} 件目：返していない確保");
        if out.error.is_some() { err += 1 } else { ok += 1 }
    }
    // 読める件も読めない件（壊れた UTF-8・`]]>` など）もある
    assert!(ok > 500 && err > 10, "読めた {ok}・読めない {err}");
}

/// 値の目印の字句（CSS の目印に足す）。
const VALUE_DICT: &[&[u8]] = &[
    b"not ",
    b" and ",
    b" or ",
    b"deg",
    b"turn",
    b"/",
    b"none",
    b"inherit",
    b"hsl(",
    b"rgb(",
    b"hwb(",
    b"matrix(",
    b"translate(",
    b"rotate(",
    b"#",
    b"%",
    b"em",
    b"vw",
    b"(min-width: ",
    b"(aspect-ratio: 1/2)",
    b"selector(",
    b"normal",
    b"oblique",
    b"!important",
];

#[test]
fn values_survive_seeded_mutations() {
    let seeds = fuzz_seeds("values");
    let refs: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    let dict: Vec<&[u8]> = CSS_DICT.iter().chain(VALUE_DICT).copied().collect();
    for (n, case) in cases(0x5eed_0008, 4000, &refs, &dict, 512).iter().enumerate() {
        let (out, heap) = measure(|| values_parse(case));
        assert_eq!(out, Check::Ok, "{n} 件目 {:?}", String::from_utf8_lossy(case));
        assert_eq!(heap.current, 0, "{n} 件目：返していない確保");
    }
}

/// 読む段の全体を途中で中止しても（数える段でも本体でも）、勘定が崩れず、確保をすべて返す。
#[test]
fn document_survives_cancellation() {
    let big = format!("<svg xmlns=\"http://www.w3.org/2000/svg\">{}</svg>", "<g a=\"1\"/>".repeat(100_000));
    let mut cancelled = 0usize;
    for calls in [0u32, 1, 20, 40, 41, 42, 43, 44, 60, 83, 84, 85, 103, 104] {
        let c = AfterCalls { left: AtomicU32::new(calls) };
        let (out, heap) = measure(|| document_parse_cancel(big.as_bytes(), &c));
        assert_eq!(out.check, Check::Ok, "{calls} 回で中止");
        assert_eq!(heap.current, 0);
        if out.error == Some(sumie::Error::Cancelled) {
            cancelled += 1;
        }
    }
    assert!(cancelled >= 8, "中止の道を通らなかった（{cancelled}）");
}

/// パスのデータの目印の字句（命令・区切り・数の書き方・弧の旗）。
const PATH_DICT: &[&[u8]] = &[
    b"M", b"m", b"L", b"l", b"H", b"v", b"C", b"s", b"Q", b"t", b"A", b"a", b"Z", b"z", b",", b" ", b".", b"-", b"+",
    b"e", b"E-", b"1e400", b"0 1 1", b"11", b"10.", b".5.5", b"\t", b"1e308", b"-1e308",
];

#[test]
fn path_survives_seeded_mutations() {
    let seeds = fuzz_seeds("path");
    let refs: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    // 種そのもの（ファズが見つけた入力を含む）と変異
    let mut all: Vec<Vec<u8>> = seeds.clone();
    all.extend(cases(0x5eed_0009, 4000, &refs, PATH_DICT, 512));
    for (n, case) in all.iter().enumerate() {
        let (out, heap) = measure(|| path_ops(case));
        assert_eq!(out, Check::Ok, "{n} 件目 {:?}", String::from_utf8_lossy(case));
        assert_eq!(heap.current, 0, "{n} 件目：返していない確保");
    }
}

#[test]
fn stroke_survives_seeded_mutations() {
    let seeds = fuzz_seeds("stroke");
    let refs: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    // 種そのものと変異
    let mut all: Vec<Vec<u8>> = seeds.clone();
    all.extend(cases(0x5eed_000a, 3000, &refs, PATH_DICT, 256));
    for (n, case) in all.iter().enumerate() {
        let (out, heap) = measure(|| stroke_ops(case));
        assert_eq!(out, Check::Ok, "{n} 件目 {:?}", String::from_utf8_lossy(case));
        assert_eq!(heap.current, 0, "{n} 件目：返していない確保");
    }
}

/// 木を作る段と描く段の変異：数・長さ・パスの命令・M2 の属性の字句だけを差し込む（構造を壊す字句を入れると、ほとんどが
/// 読めなくなって木まで届かない。単位 4 の 1 回目のレビュー）。
const SHAPE_DICT: &[&[u8]] = &[
    b"0",
    b"1",
    b"-1",
    b".5",
    b"1e30",
    b"1e-30",
    b"1e308",
    b"-0",
    b"999999",
    b"%",
    b"px",
    b"em",
    b" ",
    b",",
    b"M",
    b"L",
    b"C",
    b"Q",
    b"A",
    b"Z",
    b"m",
    b"l",
    b"c",
    b"a",
    b"z",
    b"h",
    b"v",
    b"0 1 1",
    b"none",
    b"round",
    b"square",
    b"bevel",
    b"miter",
    b"evenodd",
    b"nonzero",
    b"crispEdges",
    b"non-scaling-stroke",
    b"currentColor",
    b"#f00",
    b"0.3",
    b"2 3",
    b" stroke-width=\"0.4\"",
    b" stroke-dasharray=\"3 1\"",
    b" stroke-dashoffset=\"7\"",
    b" pathLength=\"10\"",
    b" transform=\"rotate(30)\"",
    b" transform=\"scale(1e-3 4)\"",
    b" stroke-linecap=\"round\"",
];

/// 段の確かめのヒープの照らし：山 ≤ 数えた山、確保の合計・回数 ≤ 数えた合計・回数（山でない時点の数え漏れも見つける。
/// ファズの作り直しの 2 回目のレビュー）、返していない確保がない。
fn assert_stage_heap(heap: &sumie_testkit::alloc::Heap, out: &StageOutcome, what: &str) {
    assert_heap_within(heap, out.counted_peak, 0, what);
    assert!(
        heap.total <= out.counted_total,
        "{what}：確保の合計 {} バイトが数えた {} を超える（数え漏れ）",
        heap.total,
        out.counted_total
    );
    assert!(
        heap.count <= out.counted_count,
        "{what}：確保の回数 {} が数えた {} を超える",
        heap.count,
        out.counted_count
    );
    assert_eq!(heap.current, 0, "{what}：返していない確保");
}

/// 種そのものと、構造を保つ変異（`n` 件）と、構造も壊す変異（`n / 4` 件）を通す。届いた件数（確かめの終わりまで行った
/// 件）と全体の件数を返す。種は、`unreached` に名前を挙げたもの（上限を狙って作った種で、見積もりが安全に断る）だけが
/// 届かず、ほかはすべて確かめの終わりまで届く（ファズの作り直しの 1 回目のレビュー）。
fn stage_cases(
    target: &str,
    seed: u64,
    n: usize,
    unreached: &[&str],
    run: fn(&[u8]) -> StageOutcome,
) -> (usize, usize) {
    let named = named_fuzz_seeds(target);
    let seeds: Vec<Vec<u8>> = named.iter().map(|(_, b)| b.clone()).collect();
    let refs: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    let mut all: Vec<Vec<u8>> = seeds.clone();
    all.extend(value_cases(seed, n, &refs, SHAPE_DICT, 4096));
    all.extend(cases(seed ^ 1, n / 4, &refs, SHAPE_DICT, 4096));
    // 描く段の入力は頭の 4 バイトが倍率と範囲：変異ごとに変える（種の範囲だけを見ない。単位 4 の 2 回目のレビュー）
    if target == "render" {
        let mut rng = sumie_testkit::chacha::ChaCha::from_u64(seed ^ 2);
        for case in all.iter_mut().skip(seeds.len()) {
            if let Some(head) = case.get_mut(..4) {
                rng.fill(head);
            }
        }
    }
    let mut reached = 0usize;
    let mut seeds_unreached: Vec<&str> = Vec::new();
    for (i, case) in all.iter().enumerate() {
        let (out, heap) = measure(|| run(case));
        assert_eq!(out.check, Check::Ok, "{target} {i} 件目 {:?}", String::from_utf8_lossy(case));
        assert_stage_heap(&heap, &out, &format!("{target} {i} 件目"));
        if let Some((name, _)) = named.get(i).filter(|_| !out.reached) {
            seeds_unreached.push(name);
        }
        reached += usize::from(out.reached);
    }
    assert_eq!(seeds_unreached, unreached, "{target} の種で確かめの終わりまで届かないもの");
    eprintln!("{target}：届いた件 {reached}/{}", all.len());
    (reached, all.len())
}

/// 木を作る段の種のうち、確かめの終わりまで届かないもの（見積もりか作る段が安全に断る種）。
const TREE_UNREACHED: &[&str] = &[];

/// 描く段の種のうち、確かめの終わりまで届かないもの（見積もりが上限で安全に断る種）。
const RENDER_UNREACHED: &[&str] = &[];

/// 木を作る段（`fuzzing::build_ops`）の種と種つきの変異：見積もり ≥ 実際・勘定・同じ結果・断りが見積もりと同じ・
/// ヒープの山 ≤ 数えた山・返していない確保がないこと。
#[test]
fn build_survives_seeded_mutations() {
    let (reached, n) = stage_cases("tree", 0x5eed_000b, 2000, TREE_UNREACHED, build_ops);
    assert!(reached * 2 > n, "木まで届いた件 {reached}/{n}");
}

/// 描く段まで（`fuzzing::render_ops`）の種と種つきの変異：形の約束・範囲の一致・同じ画素・ヒープの山 ≤ 数えた山・
/// 返していない確保がないこと。
#[test]
fn render_survives_seeded_mutations() {
    let (reached, n) = stage_cases("render", 0x5eed_000c, 600, RENDER_UNREACHED, render_ops);
    assert!(reached * 2 > n, "描く段の終わりまで届いた件 {reached}/{n}");
}

/// ファズと単位 4 の 1 回目のレビューが見つけた入力：見積もりが上限を超えて断る文書（太さ 2×10³⁷ の線・use の倍々 18 段）
/// と、計画が重い弧（大きな半径と T）。どれも断りが見積もりと同じで、確かめが崩れない。
#[test]
fn fuzz_found_inputs() {
    let head = "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"100\" height=\"100\">\n<rect width=\"100\" height=\"100\" fill=\"red\"/>\n";
    let wide = format!(
        "{head}<polyline points=\"20,80 50,20 80,80\" fall=\"none\" stroke=\"green\" stroke-width=\"2{}\" streko-linejoin=\"round\" stroke-miterlimit=\"2\"/>\n</svg>\n",
        "2".repeat(37)
    );
    let arc = format!("{head}<path fill=\"green\" d=\"M37 50 A1 1 0 0 1 9888888888880 50 T\"/>\n</svg>\n");
    let mut d = String::from("M0 0");
    for i in 0..300 {
        d.push_str(&format!(" L{} {}", i % 50, (i * 7) % 50));
    }
    let mut doubling = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"50\" height=\"50\"><defs><path id=\"g0\" d=\"{d}\"/>"
    );
    for l in 1..=18 {
        doubling
            .push_str(&format!("<g id=\"g{l}\"><use xlink:href=\"#g{p}\"/><use xlink:href=\"#g{p}\"/></g>", p = l - 1));
    }
    doubling.push_str("</defs><use xlink:href=\"#g18\"/></svg>");
    for (head4, svg) in [([0u8, 5, 0x37, 9], &wide), ([0, 5, 9, 0x37], &arc), ([0, 0, 0, 0], &doubling)] {
        let mut data = head4.to_vec();
        data.extend_from_slice(svg.as_bytes());
        let (out, heap) = measure(|| render_ops(&data));
        assert_eq!(out.check, Check::Ok, "{svg}");
        assert_stage_heap(&heap, &out, "ファズの入力");
        let (out, heap) = measure(|| build_ops(svg.as_bytes()));
        assert_eq!(out.check, Check::Ok, "{svg}");
        assert_stage_heap(&heap, &out, "ファズの入力");
    }
}

/// 構造から組み立てた SVG（`sumie_testkit::svg_gen`。ファズの `render_gen` と同じ生成器）を、読む段・木を作る段・描く段
/// まで通す（ADR-100 (3) (c)）：形の約束・範囲の一致・同じ画素・段ごとの見積もり ≥ 実際・ヒープの山と確保の合計・回数 ≤
/// 数えた量。生成器が届く文書を作っているかも確かめる：描く項目のある文書で描く段の終わりまで届いた割合が 7 割以上、背景の
/// ほかの画素を描いた・線がある・破線があるものがそれぞれ 6 割・5 割・3 割以上（ファズの作り直しの 1・2 回目のレビュー）。
/// fuzz/seeds/render_gen の種も通す（どれも届いて線を描く）。
#[test]
fn generated_documents_render() {
    let mut rng = sumie_testkit::chacha::ChaCha::from_u64(0x5eed_0010);
    let mut inputs: Vec<Vec<u8>> = fuzz_seeds("render_gen");
    let seeds = inputs.len();
    for _ in 0..240 {
        let len = 64 + (rng.next_u32() as usize % 960);
        let mut bytes = vec![0u8; len];
        rng.fill(&mut bytes);
        inputs.push(bytes);
    }
    let n = inputs.len();
    let (mut reached, mut drew, mut stroked, mut dashed) = (0usize, 0usize, 0usize, 0usize);
    for (i, bytes) in inputs.iter().enumerate() {
        // ファズの render_gen と同じ組み立て（頭の 4 バイトは倍率と範囲）
        let (head, rest) = bytes.split_at(bytes.len().min(4));
        let mut data = head.to_vec();
        data.resize(4, 0);
        let svg = sumie_testkit::svg_gen::svg(rest);
        data.extend_from_slice(&svg);
        let (out, heap) = measure(|| render_ops(&data));
        assert_eq!(out.check, Check::Ok, "{i} 件目 {:?}", String::from_utf8_lossy(&svg));
        assert_stage_heap(&heap, &out, &format!("生成 {i} 件目"));
        let (b, heap) = measure(|| build_ops(&svg));
        assert_eq!(b.check, Check::Ok, "木 {i} 件目 {:?}", String::from_utf8_lossy(&svg));
        assert_stage_heap(&heap, &b, &format!("生成の木 {i} 件目"));
        let (doc, heap) = measure(|| document_parse(&svg));
        assert_eq!(doc.check, Check::Ok, "文書 {i} 件目 {:?}", String::from_utf8_lossy(&svg));
        assert_heap_within(&heap, doc.peak, 0, &format!("生成の文書 {i} 件目"));
        assert!(i >= seeds || (out.reached && out.drew && out.stroked), "fuzz/seeds/render_gen の {i} 件目が届かない");
        reached += usize::from(out.reached);
        drew += usize::from(out.drew);
        stroked += usize::from(out.stroked);
        dashed += usize::from(out.dashed);
    }
    eprintln!("生成：届いた {reached}・描いた {drew}・線 {stroked}・破線 {dashed} / {n}");
    assert!(reached * 10 >= n * 7, "描く段の終わりまで届いた件 {reached}/{n}");
    assert!(drew * 10 >= n * 6, "画素を描いた件 {drew}/{n}");
    assert!(stroked * 10 >= n * 5, "線のある件 {stroked}/{n}");
    assert!(dashed * 10 >= n * 3, "破線のある件 {dashed}/{n}");
}

/// 生の SVG を読む対象（render・tree）に通す、照合の集まりの SVG（`tests/svg/` と `corpus/` の 16 KiB 以下。
/// `scripts/fuzz-corpus.sh` が流し込むものと同じ）。名前の順。
fn comparison_svgs() -> Vec<(String, Vec<u8>)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut stack = vec![root.join("tests/svg"), root.join("corpus")];
    let mut out = Vec::new();
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "svg")
                && let Ok(b) = std::fs::read(&p)
                && b.len() <= 16 * 1024
            {
                out.push((p.strip_prefix(&root).unwrap_or(&p).display().to_string(), b));
            }
        }
    }
    out.sort();
    out
}

/// 照合の集まりの SVG を、木を作る段と描く段まで通す（倍率 0.25 と 1.1。ファズの作り直しの 2 回目のレビュー）：確かめが
/// 崩れず、ヒープの山と確保の合計・回数 ≤ 数えた量、返していない確保がない。ファズの対象（libFuzzer）は確保を数えないので、
/// 生の SVG の流れのヒープの照らしはここが受け持つ。
#[test]
fn comparison_documents_render() {
    let files = comparison_svgs();
    assert!(files.len() > 1000, "照合の集まりが {} 件", files.len());
    let (mut reached, mut n) = (0usize, 0usize);
    for (k, (name, svg)) in files.iter().enumerate() {
        // 倍率は 0.25（3）と 1.1（6）を交互に、範囲もずらす
        let head = if k % 2 == 0 { [3u8, 5, 9, 0x37] } else { [6u8 | 8, 17, 3, 0] };
        let mut data = head.to_vec();
        data.extend_from_slice(svg);
        let (out, heap) = measure(|| render_ops(&data));
        assert_eq!(out.check, Check::Ok, "{name}");
        assert_stage_heap(&heap, &out, name);
        let (b, heap) = measure(|| build_ops(svg));
        assert_eq!(b.check, Check::Ok, "{name}（木）");
        assert_stage_heap(&heap, &b, &format!("{name}（木）"));
        reached += usize::from(out.reached);
        n += 1;
    }
    eprintln!("照合の集まり：描く段の終わりまで届いた {reached}/{n}");
    assert!(reached * 2 > n, "描く段の終わりまで届いた {reached}/{n}");
}

/// 育ったコーパス（git の外の `fuzz/corpus/<対象>`）を流し、確かめが崩れないこと・ヒープの山と確保の合計・回数 ≤ 数えた量・
/// 返していない確保がないことを確かめ、届き具合（描く段の終わりまで届いた・背景のほかを描いた・線がある・破線がある件数）を
/// 数えて出す（ADR-100 (3) (e)。長く回したあとに手で流す：
/// `cargo test -p sumie --release --test mutate -- --ignored corpus_reach --nocapture`。回している間に流すと、縮めて消えた
/// ファイルは飛ばす）。
#[test]
#[ignore = "git の外のコーパスを読む（手で流す）"]
fn corpus_reach() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus");
    for target in ["render", "render_gen", "tree"] {
        let Ok(rd) = std::fs::read_dir(root.join(target)) else {
            eprintln!("{target}：コーパスがない");
            continue;
        };
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        files.sort();
        let (mut n, mut reached, mut drew, mut stroked, mut dashed) = (0usize, 0usize, 0usize, 0usize, 0usize);
        for f in &files {
            let Ok(bytes) = std::fs::read(f) else { continue };
            let (out, heap) = measure(|| match target {
                "tree" => build_ops(&bytes),
                "render" => render_ops(&bytes),
                _ => {
                    let (head, rest) = bytes.split_at(bytes.len().min(4));
                    let mut input = head.to_vec();
                    input.resize(4, 0);
                    input.extend_from_slice(&sumie_testkit::svg_gen::svg(rest));
                    render_ops(&input)
                }
            });
            let what = f.display().to_string();
            assert_eq!(out.check, Check::Ok, "{what}");
            assert_stage_heap(&heap, &out, &what);
            n += 1;
            reached += usize::from(out.reached);
            drew += usize::from(out.drew);
            stroked += usize::from(out.stroked);
            dashed += usize::from(out.dashed);
        }
        eprintln!(
            "{target}：{n} 件・届いた {reached}・描いた {drew}・線 {stroked}・破線 {dashed}（描く段の印は render の対象だけ）"
        );
    }
}
