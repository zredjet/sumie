//! 段 4：木（architecture.md §4.4）。M2 の木は、描くパスの節の平らな並び（塗りと線は単色。層を持つ群・clip・mask・
//! pattern などは M3。ADR-89 (1)）。幾何は命令と座標の置き場に詰め、節は範囲で指す（use で展開した節は同じ範囲を指す）。
//! 何も借りない（`'static`・`Send + Sync`）。

pub(crate) mod build;

use crate::doc::{PixelSize, Size};
use crate::error::Error;
use crate::geom::{PathRef, Rect, Transform};
use crate::mem::{Frozen, Plain};
use crate::report::Report;
use crate::style::computed::{Axis, Base, resolve};
use crate::values::Length;

/// 節の印。
pub(crate) const HAS_FILL: u8 = 1;
pub(crate) const HAS_STROKE: u8 = 2;
/// 塗りの規則が偶奇
pub(crate) const EVEN_ODD: u8 = 4;
/// 線を先に描く（`paint-order: stroke`）
pub(crate) const STROKE_FIRST: u8 = 8;
/// 反エイリアスなし（`shape-rendering: crispEdges`・`optimizeSpeed`。ADR-91 (4)）
pub(crate) const CRISP: u8 = 16;
/// `vector-effect: non-scaling-stroke`（ADR-91 (3)）
pub(crate) const NON_SCALING: u8 = 32;
/// 破線の % の基準（表示の領域）がある
pub(crate) const HAS_VP: u8 = 64;

/// 端と継ぎ目の番号（`geom::stroke` の型に写す）。
pub(crate) const CAP_BUTT: u8 = 0;
pub(crate) const CAP_ROUND: u8 = 1;
pub(crate) const CAP_SQUARE: u8 = 2;
pub(crate) const JOIN_MITER: u8 = 0;
pub(crate) const JOIN_ROUND: u8 = 1;
pub(crate) const JOIN_BEVEL: u8 = 2;

/// 描くパスの節。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PathNode {
    /// 節の座標から根のユーザー単位（倍率 1 の描く座標系）への変形
    pub(crate) transform: Transform,
    /// 幾何の厳密な外接矩形（根のユーザー単位。線の太さを含まない）
    pub(crate) bounds: Rect,
    /// 塗りと線の色（r・g・b は 0〜255、α は 0〜1。不透明度を掛けたもの。乗算しない）
    pub(crate) fill: [f64; 4],
    pub(crate) stroke: [f64; 4],
    /// 線の太さ（節の座標。0 より大きい）・miterlimit・破線の位相・pathLength（0 ならなし）
    pub(crate) stroke_width: f64,
    pub(crate) miter_limit: f64,
    pub(crate) dash_offset: f64,
    pub(crate) path_length: f64,
    /// 破線の長さを解く文脈（要素の文字の大きさ・行の高さ・最も近い表示の領域。破線は宣言ごとに解かない長さで持つ）
    pub(crate) font_size: f64,
    pub(crate) line_height: f64,
    pub(crate) viewport: [f64; 2],
    /// 置き場の範囲（始めと長さ）：命令・座標・破線
    pub(crate) verbs: [u32; 2],
    pub(crate) coords: [u32; 2],
    pub(crate) dashes: [u32; 2],
    pub(crate) flags: u8,
    pub(crate) cap: u8,
    pub(crate) join: u8,
    pub(crate) pad: [u8; 5],
}

impl Plain for PathNode {}

/// 木。
#[derive(Debug)]
pub struct Tree {
    pub(crate) size: Size,
    pub(crate) nodes: Frozen<PathNode>,
    pub(crate) verbs: Frozen<u8>,
    pub(crate) coords: Frozen<f64>,
    /// 破線の並び（宣言ごとに 1 度、解かない長さで。use で展開した節は同じ範囲を指す）
    pub(crate) dashes: Frozen<Length>,
    /// 破線の長さを解く文書の文脈（根の文字の大きさと行の高さ・vw などの基準の文書の大きさ）
    pub(crate) root_font: f64,
    pub(crate) root_line: f64,
    pub(crate) doc_size: Option<(f64, f64)>,
    pub(crate) report: Report,
}

impl Tree {
    /// ユーザー単位の大きさ（中身で決まる文書は中身の外接矩形の右下。FR-OUT-01）。
    pub fn size(&self) -> Size {
        self.size
    }

    /// 倍率 `scale` の画素の大きさ（FR-OUT-01・ADR-78 (5)）。
    pub fn pixel_size(&self, scale: f64) -> Result<PixelSize, Error> {
        crate::doc::size::to_pixels(self.size, scale)
    }

    /// この段までの警告・読まなかった参照。
    pub fn report(&self) -> &Report {
        &self.report
    }

    /// 木が持ち続ける量。
    pub fn held_bytes(&self) -> u64 {
        [self.nodes.bytes(), self.verbs.bytes(), self.coords.bytes(), self.dashes.bytes(), self.report.bytes()]
            .iter()
            .fold(0u64, |a, &b| a.saturating_add(b))
    }

    /// 節のパス。
    pub(crate) fn path(&self, n: &PathNode) -> PathRef<'_> {
        let range = |r: [u32; 2]| (r[0] as usize)..(r[0] as usize).saturating_add(r[1] as usize);
        PathRef {
            verbs: self.verbs.as_slice().get(range(n.verbs)).unwrap_or(&[]),
            coords: self.coords.as_slice().get(range(n.coords)).unwrap_or(&[]),
        }
    }

    /// 節の破線の並び（解かない長さ。なければ空）。
    pub(crate) fn dash_list(&self, n: &PathNode) -> &[Length] {
        let start = n.dashes[0] as usize;
        self.dashes.as_slice().get(start..start.saturating_add(n.dashes[1] as usize)).unwrap_or(&[])
    }

    /// 破線の長さを節の文脈で解く（節の座標のユーザー単位。% は表示の領域の対角を √2 で割ったものに対して。解けなければ
    /// None：呼び手は破線を実線にする）。
    pub(crate) fn dash_value(&self, n: &PathNode, l: Length) -> Option<f64> {
        let base = Base {
            default_font: self.root_font,
            root_font: self.root_font,
            root_line: self.root_line,
            viewport: (n.flags & HAS_VP != 0).then_some((n.viewport[0], n.viewport[1])),
            doc: self.doc_size,
            outermost: false,
        };
        resolve(l, n.font_size, n.line_height, &base, Axis::Diag)
    }
}
