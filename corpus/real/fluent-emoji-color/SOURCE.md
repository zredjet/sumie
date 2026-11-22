# 出どころ：fluent-emoji-color

| 項目 | 値 |
|---|---|
| リポジトリ | https://github.com/microsoft/fluentui-emoji |
| 版 | コミット `1ffb34c752ecf5d402f04cfb4b392c77f57c54bc` |
| 取ったもの | `assets/` の下（道に `/Color/` を含むもの）の `.svg`（16 MiB 以下）。当てはまる 3145 件から 120 件を種つきで抜き取り（ChaCha8・種 1）。合わせて 4754007 バイト |
| 取り方 | `https://raw.githubusercontent.com/microsoft/fluentui-emoji/1ffb34c752ecf5d402f04cfb4b392c77f57c54bc/<道>` から取り、tree の一覧の git の blob の SHA-1 を照らした |
| ライセンス | MIT（`LICENSE` はリポジトリの根の写し） |
| 性質 | 放射・線形のグラデーションの多い絵文字 |
| 一覧 | `MANIFEST.jsonl`（道・バイト数・BLAKE3・git の blob） |

作り直し：`cargo run -p sumie-tools -- corpus fetch-real --only fluent-emoji-color`（ADR-59）
