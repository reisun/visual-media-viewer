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
| ↑ ↓ | 前/次の兄弟フォルダ |
| PgUp / PgDn | 親フォルダ / 子フォルダ |
| Space | 動画 再生/一時停止 |
| R / Shift+R | 回転（時計回り/反時計回り） |
| S | スライドショー切替 |
| +/- | スライドショー間隔調整 |
| マウスホイール | ズーム（画像） / 音量（動画） |
| 右クリックドラッグ | ズーム |
| ダブルクリック | 表示リセット |
| タイトルバー右クリック | メニュー（並び順/表示モード） |

## ビルド

Docker ベースのクロスコンパイル（Linux → Windows x86_64）:

```bash
./scripts/build.sh release
```

出力: `dist/VisualMediaViewer.exe`

### 必要環境

- Docker / Docker Compose
- WSL2（推奨）

## ライセンス

MIT License - 詳細は [LICENSE](LICENSE) を参照。

### サードパーティ

- **FFmpeg** (LGPL 2.1) - 動的リンク（DLL同梱）
- **libjpeg-turbo** (BSD-3-Clause) - 静的リンク

詳細は [THIRD_PARTY_LICENSES.txt](licenses/THIRD_PARTY_LICENSES.txt) を参照。
