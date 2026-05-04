# TASK: visual-media-viewer

## 概要
MassiGraを超える高レスポンス画像・動画ビューア。Rust + wgpu + eguiによるフルネイティブGPU描画。

## プロジェクト情報
- リポジトリ: GitHub（reisun/visual-media-viewer）
- 技術スタック: Rust + wgpu + egui + FFmpeg（LGPL DLL動的リンク）
- ビルド環境: Docker内クロスコンパイル（WSL2 → Windows x86_64）
- CI: なし（ローカルDockerビルド → 手動リリース）

## 完���済み

### Phase 1: 画像ビューア MVP ✅
画像表示（JPEG/PNG/GIF/BMP）、GPUレンダリング、ファイルナビゲーション（名前順ソート）、プリロード・LRUキャッシュ、ズーム（ホイール/右クリックドラッグ/ダブルクリックリセット）

### Phase 2: 機能拡張 ✅
自然順ソート、ループナビゲーション、WebP/TIFF対応、画像回転、スライドショー

### Phase 2.5: UI オーバーホール ✅
カスタムタイトルバー、タイトルバー右クリックメニュー（リスト/表示）、並び順（名前/更新日時）、フィット表示モード、フォルダ移動（↑↓兄弟/PgUp親/PgDn子）、グループ化

### Phase 3A: HEIC対応・パン改善 ✅
HEIC/HEIF対応（Windows WIC API経由）、WICフォールバック、拡大時マウス位置連動パン

### Phase 4: 動画再生対応 ✅
FFmpeg LGPL DLLリンク、動画フォーマット対応（MP4/WebM/MKV/AVI/MOV/WMV/FLV/M4V/MPG/MPEG/TS）、バックグラウンドデコード、PTS同期、再生/一時停止、画像・動画混在ナビゲーション

### Phase 5: 動画再生品質改善・音声同期 ✅
音声再生（cpal + FFmpegデコード + リサンプラー）、シーク（±10秒/シークバー）、音量調整（0-200%）、IPC多重起動防止、アプリアイコン、音声映像同期（awaiting_first_frame/seeking/audio_paused）、マルチスレッドデコード、解像度キャップ（1920x1080）、プリバッファリング、ローディングスピナー

### Phase 5.5: UX改善 ✅
ウインドウサイズ・最大化状態の記憶・復元

### 安定化修正 ✅
- スキップ/シーク時UIフリーズ解消（stop_flag + read-ahead throttle）— PR #12
- 1分再生停止修正（VecDequeバッファリング + 音声EOF検出）— PR #13
- メモリ使用量改善（画像キャッシュ: バイトベース512MB / 動画: sync_channel(4), prebuffer(8), 音声スロットル5秒上限）— PR #14, #15
- 音量カーブ改善（リニア→二乗）、IPC時ウインドウフォーカス修正、動画再生診断ログ追加 — PR #16
- bounded channelバックプレッシャー（unbounded channel + clock sleepスロットル → sync_channel自然フロー制御）— PR #19

## 課題（対応中）

- [ ] 再生されない動画がある — PR #16でflags転送と診断ログを追加済み、次回ログで原因特定予定

## バックログ

- [ ] Explorerサムネイル表示 — IThumbnailProvider COM DLLが必要（別コンポーネント、中〜大規模）
- [ ] 動画ループ再生
- [ ] GIFアニメーション再生
- [ ] GPUデコード検討（重い動画の最適化）
- [ ] EXIF プロパティ表示
