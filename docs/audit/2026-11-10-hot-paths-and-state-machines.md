# 非同期処理・状態遷移・テストの整理

README の本文取得、Slack のチャンネル名解決、CDP のサブリクエスト検査で、独立した処理を逐次に待つ箇所を修正した。Slack では、存在しないメッセージの名前解決が遅いと NotFound ではなく Timeout を返す不具合も修正した。

対象は `src` の製品コードと `tests`。6コマンドの実行から HTTP、DNS、ブラウザ、変換、出力までを静的に調べ、既存テストと追加テストで確認した。依存ライブラリ内部とテスト用サーバーの状態は、以下の一覧に含めない。

## 修正と検証

| 対象 | 修正前の問題 | 修正後の振る舞いと証拠 |
| --- | --- | --- |
| `tools/repo.rs` | README のメタデータを取得しても、issues・pulls・releases の完了まで blob を取りに行かない | `fetch_readme` が取得・blob 補完・デコード・劣化理由をまとめ、他の3セクションと並行して完了する。T-HP001、T-TS010で確認 |
| `slack/client.rs` | 履歴・返信の全取得後にチャンネル名を解決する | チャンネル名と、履歴・返信・ユーザー名の一連の解決を並列化した。後者が失敗すればチャンネル名の待機も中止する。T-HP002で確認 |
| `slack/client.rs` | 対象が存在するかをユーザー名解決後に調べ、遅い名前解決で NotFound が Timeout に変わる | 名前解決前に対象の位置を確定する。`ResolvedThread` が対象・文脈・劣化情報を保持する。T-HP003、T-HP005で確認 |
| `fetch/cdp/connection.rs` | 1件の DNS 検査中は次のサブリクエストを処理しない | 検査を最大5件まで並行して進める。各検査が終わるまで通信を許可せず、interceptor の応答も待つ。追加タスクは起動しない。T-HP004と既存の許可・遮断・interceptor 失敗テストで確認 |

T-HP001〜004は、製品コードの修正前に4件とも失敗し、修正後に通過した。HTTP のテストでは、必要な別リクエストが届くまで応答を保留する。同時実行の検証に、処理時間の短縮率や短い sleep を使っていない。

`Scout::new` は待機する処理を持たないため同期関数にした。検索用の `SearchClient` trait と `MockSearch` は削除し、`BraveClient` を直接使う。research の順位復元は、取得結果を一度ソートしてから成功・失敗へ分ける処理にまとめた。ページ取得の上限付き並列実行は維持している。

## 確認した状態遷移

ここでの状態遷移は、前の入力や完了を保持し、後続の処理を変えるものを指す。単発の値変換やエラー分類表は含めない。明示的な enum として実装済みのものも、関連する暗黙の状態と一緒に記載する。パスは `src/` からの相対パス。

| 場所・所有者 | 状態と遷移 | 維持する条件 |
| --- | --- | --- |
| `tools.rs`: `StdinResolver` | 未読 → 利用可能 → 消費済み | stdin を複数の位置引数に二重供給しない |
| `token_source.rs`: トークン解決 | GITHUB_TOKEN → GH_TOKEN → gh 実行 → 認証済み／認証なし | 前の方法で取れたら後続を実行しない。gh の時間切れ・失敗は認証なしへ進む |
| `tools.rs`: GitHub／Slack の `OnceCell` | 未初期化 → 初期化中 → 初期化済み | GitHub の非同期トークン解決は共有する。Slack の初期化失敗はキャッシュしない |
| `lib.rs`: `drive` | コマンド実行中 → 完了、または signal → cancel 通知 → drain 完了／打切り → signal 終了 | drain は7秒まで。signal の終了コードを通常のコマンド結果で上書きしない |
| `tools.rs`・`cdp.rs`: cancel watch | 未通知 → 通知済み | 通知後に購読した取得も中断を認識する |
| `retry.rs` | 試行 → 成功／終端エラー、または再試行可能エラー → 待機 → 次の試行 | 試行は逐次。最後の試行はその結果を返し、追加で待機しない |
| `tools/repo.rs`: repo-tree | 入力確定・検証 → ref 指定済み、またはデフォルトブランチ取得 → tree 取得 → 絞込み | ref の取得結果が必要な tree 呼出しは逐次 |
| `tools/repo.rs`: repo-read | 入力確定・検証 → contents → 本文あり／blob 補完／ディレクトリ／404 | 本文のないファイルだけ blob を補完する。404 の候補検索は表示できるときだけ行う |
| `tools/repo.rs`: repo-overview | repo 確認 → README・issues・pulls・releases → 出力 | repo 自体の404では後続 API を呼ばない。任意セクションの失敗は劣化として出力する |
| `tools/repo.rs`: `fetch_readme` | README → 本文あり／blob → デコード、または欠落／取得失敗／デコード失敗 | README の404は無言の欠落。他の失敗は段階別の劣化理由を残す |
| `tools/repo.rs`・`tools/typo.rs`: 候補検索 | ref 解決 → tree → 上限確認 → 距離計算・残作業量減算 → 候補／省略 | API 待機と計算を同じ時間枠で制限する。上限超過時は全候補を省略する |
| `search/engine.rs`: research | 検索 → 上位 depth 件を最大5件取得 → 順位復元 → レポート | 検索前に URL は分からない。完了順で表示順を変えない |
| `tools/query.rs`: research の結果 | 通常レポート／再試行可能な検索失敗による劣化／終端エラー／時間切れ | 検索失敗と正常な0件を区別する |
| `fetch.rs`: ページ取得 | URL 検査 → download → 明示的な非HTMLテキスト／HTML → JS 必要判定 → 抽出 → 薄い抽出の再取得 → 変換 | 明示的な JS の失敗は返す。自動 JS の失敗は元の HTML・抽出を使う。既に JS を試した本文を再度薄い抽出経由で取得しない |
| `fetch/download.rs`: リダイレクト | 現在 URL → 応答 → Location 解決・次 URL 検査 → 次 hop、または最終本文 | 次の URL は応答依存。各 hop の検査と接続時 IP 検査を維持する |
| `body_limit.rs`: 本文読込み | Content-Length 確認 → chunk の累積 → EOF／上限超過／通信失敗 | 圧縮展開後のサイズも制限する。snippet は必要なバイト数で止める |
| `fetch/download.rs`: 文字コード | ラベル指定のデコード → 信頼できる自動判定 → 不確かな UTF-8 復元 | 本文と不確かさを一緒に返す |
| `github/encoding.rs`: ファイルの文字コード | 明示指定 → BOM → 自動判定 → UTF-8、またはエラー | 明示指定や BOM がある場合は、失敗しても別の文字コードへ黙って進まない |
| `fetch.rs`: 薄い本文の走査 | タグ外／タグ内、タグ名読取り中、script・style 本文のスキップ、空白の連続 | タグ文字列や script・style の中身を本文量として数えない |
| `fetch/cdp/launch.rs`: バイナリ探索 | PATH の候補を順に検索 → 既知のパス → NotFound | 候補の優先順位を維持する。同期の which 探索は今回変更していない |
| `fetch/cdp.rs`: ブラウザ取得 | proxy 起動 → Chromium 起動 → DevTools URL → 接続 → navigate／cancel → close → reap | 起動順と close・process group 停止・profile 削除の順を維持する |
| `fetch/cdp/launch.rs`: `ChromiumProcess` | group armed → TERM → 猶予 → KILL／ESRCH → disarmed → wait | Future の破棄時も Drop が group を停止する。KILL 失敗なら Drop の再試行を残す |
| `fetch/cdp/connection.rs`: ページ | target 作成 → session attach → Page・lifecycle・Fetch 有効化 → navigate → 対応する load → DOM → HTML | frameId と loaderId が一致する load だけを完了とする。ソケットの初期化コマンドは既存の順序を維持する |
| `fetch/cdp/connection.rs`: コマンドと検査 | コマンド送信 → reply 待ち。割込みリクエストは検査待ち → 許可／遮断コマンド → interceptor reply | 対応する session・id の reply と、進行中の検査・interceptor の完了を待つ。失敗を見落とさない |
| `fetch/cdp/proxy.rs`・`transport.rs`: SOCKS5 | greeting → CONNECT・宛先 → DNS・IP 検査 → dial → 応答 → 双方向転送 → 切断 | 検査した IP だけに接続する。別接続は既に別タスクで動く |
| `fetch/cdp/proxy/transport.rs`: listener | accept → 接続タスク起動 → accept。accept 失敗 → backoff、中断 → 停止 | 接続1件の転送完了を待って次を accept する構造ではない |
| `slack/client.rs`: メッセージ取得 | thread_ts 指定なら replies。未指定なら history → reply_count に応じ replies／単独メッセージ | reply_count と cursor の後続取得は応答依存 |
| `slack/client.rs`: replies | cursor なし → ページ取得 → ts 重複除去 → 次 cursor／完了／ページ上限 | ページ間で親が重複しても一度だけ返す。上限に残返信があれば truncated を残す |
| `slack/client.rs`: 対象と名前解決 | 対象位置の確定 → 著者を出現順に収集 → mention を収集 → 最大50人を最大5件ずつ解決 → 対象・文脈を整形 | 著者を優先し、兼用 ID は一枠とする。対象の欠落を名前解決より前に返す |
| `slack/mention.rs` | 次の mention 開始 → 終端・ID・label 確認 → span、またはスキップ → 次の開始位置 | 不正な mention が本文やユーザー枠を消費しない |
| `envelope.rs`・`tools/query.rs` | 正常 → 劣化理由の累積 → Markdown の注記・JSON の degraded | 元本文と、名前解決・抽出・文字コード・切詰めによる劣化を区別して返す |
| `fetch/converter.rs` | raw-text タグ開始 → 属性の引用内／外 → タグ終端 → raw-text 終端検索 → 次のタグ | 引用内の > や script の文字列をタグ境界と取り違えない |
| `fetch/converter/html.rs`・`html/table.rs` | 通常／preformatted の再帰、兄弟間の改行・空白、リストの行頭・字下げ、表ヘッダ未決定／決定済み | pre と表セルの空白規則、表の列数、ヘッダ選択を保持する |
| `markdown.rs` | フェンス外／内と delimiter の文字・幅、frontmatter／本文、setext 下線のスキップ、連続 backtick の長さ | 広いフェンスを短い delimiter で閉じない。本文の見出し変換でメタデータやコードを変えない |
| `yaml.rs` | フェンス開始位置・閉鎖状態 → marker 保護／書換え → 切詰めで露出した末尾の再処理 → 必要な close | 合成した close で元の不閉鎖フェンスを隠さない |
| `markdown.rs`・`slack/format.rs` | CommonMark の block depth・フェンス範囲 → close／dangling、Slack は予算内へ切詰め位置を戻して再観測 | レポートの次セクションや返信が前のコードブロックへ入らない。CR・tab の置換で source offset を保つ |

## テストの整理

既存23件を削除・置換し、振る舞いを検査する9件を追加した。削除対象は、モック検索クライアント、内部の順位分割関数、対象メッセージ抽出関数、ポインタ・OnceCell の状態、serde の単純な往復、同じ検索呼出し数の重複、仮の外部ドメインに依存する research のテスト。T-TS010の並列性テストも、生 TCP サーバーの分岐をテスト本体へ書く方式から、応答データと待機条件を宣言する fixture に変えた。

新しいコマンドテストは `Scout::run` を呼び出し、Markdown、JSON、終了コード、実際の外部リクエストを確認する。認証の不要な起動と Slack トークン欠落は、実バイナリの CLI で確認する。CLI の作業用ディレクトリに、既に依存している tempfile を dev-dependency として使った。

外部 API 全体をライブサービスで検査すると、認証情報、レート制限、実データに依存する。そのためローカル HTTP・WebSocket の応答 fixture は残した。DNS の待機を再現するための resolver と、時刻・乱数の固定も境界で使っている。純粋な変換や通信プロトコルの既存テストは、独自の振る舞いを検査しているものを維持した。

## 検証結果と限界

- 通常構成の nextest は824件、全機能・ignored を含む構成は850件通過。両方で `SCOUT_NETWORK_TESTS=1` を指定し、skip は0件だった。
- fmt、Clippy の通常・全機能構成、cargo-deny、cargo-audit、cargo-machete を実行した。deny には従来の依存バージョン重複と未使用のライセンス許可の警告がある。
- Chrome による取得、ブラウザの子孫プロセス・profile の終了処理も既存テストで通過した。
- 実運用の速度や負荷は測定していない。同時進行できることと出力の正しさを確認したもので、性能改善率を示す結果ではない。
- 先のパッケージ更新による `Cargo.lock` の変更を保持し、今回の整理では lockfile を変えていない。

## 設計記録と現行コードの相違

DR-0008の採用時の説明には、async な `Scout::new` のシグネチャを保持する条件、ポインタ同一性のテスト、当時の MSRV 1.95 が記載されている。今回の指示に合わせ、内部コンストラクタは同期にし、テストは振る舞いへ移した。現行の MSRV は `Cargo.toml` の1.99.1。外部の `scout::run` は引き続き async である。

DR-0005の移行手順には `SearchClient` trait、DR-0003の過去の実装注記には `resolve_readme` が記載されている。今回、前者は削除し、後者は取得全体を持つ `fetch_readme` に置き換えた。過去の移行記録そのものは書き換えていない。

DR-0017には、drain 打切り後のブラウザを OS reaper に委ねる前提の説明が残る。現行の正常な Drop 経路は DR-0032と `ChromiumProcess` の実装どおり、process group に SIGKILL を送ってから profile を削除する。今回、この終了順序は変更していない。

追記 (2026-11-10): この節の DR-0008 と DR-0017 の相違は、各 DR に同日付の追補を入れて解消した。DR-0003 には variant 数と `unknown()` についての追補を入れた。DR-0005 の `SearchClient` の記述は移行記録として残している。
