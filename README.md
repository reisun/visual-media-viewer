# Visual Media Viewer

高レスポンス画像・動画ビューア。Rust + wgpu + egui によるフルネイティブGPU描画。

## 特徴

- **GPU描画**: wgpu によるハードウェアアクセラレーション描画
- **高速画像表示**: GPU mipmap生成、JPEG半サイズデコード（libjpeg-turbo）、テクスチャ事前アップロード
- **幅広いフォーマット**: JPEG / PNG / GIF / BMP / WebP / TIFF / HEIC
- **動画再生**: MP4 / WebM / MKV / AVI / MOV / WMV 等（FFmpeg LGPL）
- **音声同期**: cpal + FFmpegデコード + リサンプラー
- **軽快なナビゲーション**: 自然順ソート、ループ、フォルダ移動、プリロード
- **UI**: カスタムタイトルバー、ズーム・パン・回転、スライドショー

## インストール

[Releases](https://github.com/reisun/visual-media-viewer/releases) から zip をダウンロードして展開。`VisualMediaViewer.exe` を実行。

FFmpeg DLL は同梱されています。

## 操作方法

| 操作 | 機能 |
|------|------|
| ← → | 前/次の画像（動画: ±10秒シーク） |
| ↑ / ↓ | 前/次の画像フォルダ |
| Shift+↑ / Shift+↓ | ひとつ上の階層で前/次の sibling branch へ移動 |
| PgUp / PgDn | 画像: ±5ファイル（端で止まり、端で再押下時のみループ） / 動画: ±5分シーク |
| Space | 動画 再生/一時停止 |
| R / Shift+R | 回転（時計回り/反時計回り） |
| S / Shift+S | スライドショー開始 / 停止 |
| S+D / S+F | スライドショー間隔 +0.1秒 / -0.1秒 |
| +/- | スライドショー間隔調整 |
| N | タイトルバーの root 表示を現在フォルダにリセット |
| F12 | diagnostics.txt 出力 |
| マウスホイール | ズーム（画像） / 音量（動画） |
| 右クリックドラッグ | ズーム |
| ダブルクリック | 表示リセット |
| タイトルバー右クリック | メニュー（並び順/表示モード） |

### タイトルバー表示

- 起点フォルダは現在フォルダから開始し、フォルダ移動のたびに cumulative LCA を root として再計算します
- 表示形式は `root/relative-child/file (index / total)` です
- `N` を押すと root を現在フォルダに戻せます

### スライドショー

- 保存されるのは間隔だけで、起動時は常に OFF です
- 手動でのファイル移動に成功するとタイマーはその時点から再スタートします
- 動画再生中は間隔タイマーを無視し、`PlaybackState::Finished` になった時点で 1 回だけ次ファイルへ進みます

## ビルド

Docker ベースのクロスコンパイル（Linux → Windows x86_64）:

```bash
./scripts/build.sh release
```

出力: `dist/VisualMediaViewer.exe`

### 必要環境

- Docker / Docker Compose
- WSL2（推奨）

## 既知の課題

- 一部の動画が再生されない場合がある（診断ログ対応済み、原因調査中）
- GIFアニメーション未対応（静止画として表示）
- 動画のループ再生未対応

## ライセンス

MIT License - 詳細は [LICENSE](LICENSE) を参照。

### サードパーティ

- **FFmpeg** (LGPL 2.1) - 動的リンク（DLL同梱）
- **libjpeg-turbo** (BSD-3-Clause) - 静的リンク

詳細は [THIRD_PARTY_LICENSES.txt](licenses/THIRD_PARTY_LICENSES.txt) を参照。
