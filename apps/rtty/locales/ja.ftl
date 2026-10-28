menu-file = ファイル
menu-view = 表示
menu-settings = 設定
menu-help = ヘルプ
menu-zoom-in = 拡大
menu-zoom-out = 縮小
menu-zoom-reset = 拡大率をリセット ({ $percent }%)
menu-open-config = 設定フォルダーを開く
menu-quit = 終了
menu-language = 言語
menu-station = 自局情報...
menu-manual = マニュアル

window-scope = スコープ

input-device = 入力デバイス
output-device = 出力デバイス

section-tuning = 同調
section-transmit = 送信操作

label-mark = マーク
label-shift = シフト
label-speed = 速度
label-signal = 信号
label-spectrum = スペクトラム
label-channels = マーク / スペース

action-reverse = 反転
action-afc = AFC
action-squelch = スケルチ
action-adopt-tones = 検出した周波数を採用
action-scope = スコープ
action-clear = 消去
action-resync = 再同期
action-unshift-on-space = スペースで文字符号に戻す
action-atc = しきい値自動補正

path-resonator = IIR + 多数決

case-letters = 文字
case-figures = 数字

status-audio = { $rate } Hz
status-no-audio = 入力デバイスなし
status-no-output = 出力デバイスなし
status-dropped = { $samples } サンプル欠落
status-reading = 読み込み中
status-rig = リグ: { $state }
status-keyer = FSK キーヤー: { $address }

hint-listening = まだ何も受信していません。受信機を合わせるか、WAV ファイルをここにドロップしてください。
hint-resync = 同期を失った行のために、フレーミングをやり直します
hint-scope = 描画できる信号がありません。

error-config = 設定を読み込めませんでした
error-device-lost = 入力デバイスが停止しました
error-open-folder = フォルダーを開けませんでした
error-open-manual = マニュアルをブラウザーで開けませんでした
error-wav = 録音を読み込めませんでした

label-on-air = 送信中
label-queued = 送信待ち
label-level = 出力
label-templates = 定型文

action-send = 送信
action-stop = 中止

status-remaining = 残り { $seconds } 秒

hint-squelch = この値を超えた信号だけを印字します。0 で無効になります。
hint-stop = 送信を中止し、送り終えていない部分を本文に戻します
hint-drop-queued = この本文を送信待ちから外します
hint-templates = templates.toml に登録した定型文を本文に書き込みます。先頭 9 件は Ctrl+F1〜Ctrl+F9 でも選べます。
hint-unsendable = { $character } はボドー符号にありません。取り除いてから送信してください。

error-no-output = 出力デバイスが選択されていません
error-rig-unavailable = リグコントロールが使用できません: { $error }
error-underrun = サウンドカードへの供給が間に合わず、送信内容に欠落が生じました

section-contact = 交信相手

label-my-call = 自局コール
label-my-name = 自局名
label-my-qth = 自局 QTH
label-my-grid = 自局グリッド
label-his-call = 相手コール
label-his-name = 相手名
label-his-qth = 相手 QTH
label-rst-sent = 送信 RST
label-rst-received = 受信 RST

action-clear-contact = 相手情報を消去

hint-clear-contact = 交信相手の欄を空にし、次の局に備えます

station-title = 自局情報
station-close = 閉じる
station-callsign-required = コールサインはすべてのマクロが署名に使います。名前と QTH はそれらを含むマクロに反映されます。

custom-title = 追加フィールド
custom-name = 名前
custom-value = 値
custom-add = フィールドを追加
custom-invalid = 名前には英数字とアンダースコアが使えます。各区切りは英字で始めてください。
custom-note = ここで付けた名前は、マクロ中で ${ "{" }custom.名前{ "}" } と書きます。値は電波に乗るため、送信できる文字である必要があります。

action-logger-send = ロガーと連携する
logger-synced = ロガーの入力フォームと同期中
error-logger = 相手局情報をロガーへ送れませんでした

contact-title = 相手局
contact-open = この局についてディレクトリが持っている情報
contact-note-keys = マクロからは ${ "{" }contact.name{ "}" } で参照します。ここで入力した値は取り込みで上書きされません。
contact-other = その他の項目
contact-add = 追加
contact-state-looking = 照会中…
contact-state-unknown = { $callsign } の情報はありません。
contact-state-failed = 相手局ディレクトリを読めませんでした: { $detail }
contact-name = 名前
contact-name-latin = 名前 (ローマ字)
contact-qth = QTH
contact-qth-latin = QTH (ローマ字)
contact-grid = Grid
contact-jcc = JCC/JCG
contact-dxcc = DXCC
contact-dxcc-id = DXCC No.
contact-cq-zone = CQ ゾーン
contact-itu-zone = ITU ゾーン
contact-continent = 大陸
contact-state = State
contact-county = County
contact-iota = IOTA
contact-qsl-manager = QSL Via
contact-email = メール
contact-note = メモ


rig-state-disconnected = 未接続
rig-state-connecting = 接続中…
rig-state-receiving = 接続済み
rig-state-transmitting = 送信中
rig-state-failed = 接続失敗
