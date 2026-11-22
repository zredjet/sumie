# 自作の試験の SVG

このリポジトリのために書いた SVG（ライセンスはリポジトリと同じ MIT OR Apache-2.0）。ゴールデンの入力にも使う
（`crates/sumie/goldens/README.md`）。取り込んだ照合の集まりは `corpus/`（出どころつき）。

| フォルダ | 中身 |
|---|---|
| `m0/` | 照合の道具を試す小さな例（大きさの決め方：width・height・viewBox だけ・どちらもない。`emoji.svg` は Kura の設定の resvg が絵文字を描かないことの確かめ（ADR-56 (2)。`sumie-tools` の無視にしてある試験 `emoji_is_not_drawn_by_resvg`）） |
