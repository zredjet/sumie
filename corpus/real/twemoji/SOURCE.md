# 出どころ：twemoji

| 項目 | 値 |
|---|---|
| リポジトリ | https://github.com/jdecked/twemoji |
| 版 | コミット `b6b55fef1e8636b540a6d016a4729ca8cdf2e60b` |
| 取ったもの | `assets/svg/` の下の `.svg`（16 MiB 以下）。当てはまる 4009 件から 300 件を種つきで抜き取り（ChaCha8・種 3）。合わせて 777172 バイト |
| 取り方 | `https://raw.githubusercontent.com/jdecked/twemoji/b6b55fef1e8636b540a6d016a4729ca8cdf2e60b/<道>` から取り、tree の一覧の git の blob の SHA-1 を照らした |
| ライセンス | 絵は CC BY 4.0（LICENSE-GRAPHICS）・コードは MIT（LICENSE）（`LICENSE`・`LICENSE-GRAPHICS` はリポジトリの根の写し） |
| 性質 | svgo で詰めた平坦な path の絵文字 |
| 帰属 | Twemoji（2014〜2021 年は Twitter, Inc、2022 年からは Jason Sofonia と Justine De Caires（jdecked/twemoji）と、ほかの貢献者による）。CC BY 4.0（https://creativecommons.org/licenses/by/4.0/）で使い、取ったバイト列のまま変えていない |
| 一覧 | `MANIFEST.jsonl`（道・バイト数・BLAKE3・git の blob） |

作り直し：`cargo run -p sumie-tools -- corpus fetch-real --only twemoji`（ADR-59）
