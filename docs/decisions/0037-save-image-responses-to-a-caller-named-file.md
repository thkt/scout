---
status: "proposed"
date: 2026-11-12
---

# 画像の応答を呼び出し側が指定したファイルへ保存する

## Context and Problem Statement

[Issue #547](https://github.com/thkt/scout/issues/547) は、AI エージェントが Web 上の画像を OCR や要約を挟まず画像入力として受け取ることを求めている。基準版は `0ec7e876e2724a29fe29a6b8e807e850c72f1a0a`（v2.7.2）。基準版の `check_content_type`（`src/fetch/download.rs`）は `image/*` を `UnsupportedContentType` で拒否し、`scout fetch` は終了 65 になる。scout の出力は標準出力のテキストだけで、画像のバイト列をエージェントの画像入力へ渡す経路が無い。

[DR-0034](0034-explicit-non-html-text-xml-fetch.md) と [DR-0029](0029-explicit-plain-text-fetch.md) は受理範囲を text と XML に限り、Reassessment Triggers に受理範囲の変更と他媒体の忠実な出力を挙げている。本記録はその再評価に当たる。両 DR の本文は変更しない。

## Decision Drivers

- 画像を変換せずに届ける。`.claude/OUTCOME.md` は、中間 LLM の要約ではなく一次ソースそのものをエージェントへ届ける状態を目標にしている。
- MCP サーバ提供は `.claude/OUTCOME.md` の Non-goals にある。
- 全 fetch 経路で SSRF 防御を必須とする（[DR-0001](0001-ssrf-defense-architecture-and-fetchrs-module-structure.md)）。
- 保存先は、呼び出し側エージェントの sandbox が読める場所でなければならない。

## Considered Options

- 呼び出し側が `--output <path>` で指定したファイルへ保存する
- scout が一時ディレクトリを選んで保存し、そのパスを返す
- MCP サーバとして画像の content block を返す
- scout の中で OCR またはキャプション生成を行い、テキストで返す
- ページ内の全画像を一括で保存する

## Decision Outcome

Chosen option: 「呼び出し側が `--output <path>` で指定したファイルへ保存する」。CLI の標準出力を保ったまま画像のバイト列を渡せるのはファイルだけで、保存先を呼び出し側が決めれば sandbox の読み取り範囲と食い違わない。

`--output` の有無が経路を選ぶ。`--output` がある fetch は `fetch_image`（`src/fetch.rs`）を通り、無い fetch と `research` は `fetch_page` を通り続ける。

| 項目 | 決定 |
| --- | --- |
| 受理する媒体 | `ImageFormat::from_content_type`（`src/fetch/download.rs`）が認める `image/png`、`image/jpeg`、`image/gif`、`image/webp` の 4 種。`image/svg+xml` はマークアップであり、画像読み込みツールがラスタライズしないので含めない |
| 取得 | `open_image` と `download` は `final_response` を共有する。リダイレクトごとの `ssrf_check` と成功ステータスの確認はこの 1 箇所にある。`open_image` はヘッダで媒体を判定し、`ImageResponse::read` が本文を `MAX_RESPONSE_BYTES` を上限に `read_body_capped` で読む。文字コードの変換はしない |
| 中身の照合 | `ImageFormat::matches_signature` が本文の先頭バイトを Content-Type の形式と照合する。一致しなければ `FetchError::ImageSignatureMismatch`（終了 65）で、ファイルを作らない |
| 保存先の事前確認 | `ensure_new_file_can_be_created`（`src/tools/save.rs`）が取得の前に、保存先が既にあること、親ディレクトリが無いことを終了 74 で拒否する。書けない保存先のために最大 10,000,000 バイトを読まない |
| 書き出し | `save_new_file` が `create_new` でファイルを開く。取得の間にファイルシステムが変わりうるので、上書きしないことを決めるのはこの操作である。既存ファイル、親ディレクトリ不在、権限なし、書き込みの失敗は `ScoutError::io_error`（終了 74）になる。書き込みの後に `sync_all` を呼び、キャッシュへの書き込みだけが成功して flush で失敗する場合を取りこぼさない。失敗した場合は、その実行が作ったファイルを削除する |
| 保存先のパス | UTF-8 に限る。`FetchParams::output` を `String` にして、UTF-8 でないパスを clap に使用法エラー（終了 64）で拒否させる。frontmatter と JSON は UTF-8 でないバイト列を元のまま返せず、返したパスが実在しなくなる |
| 割り込み | `Scout::fetch_image_to`（`src/tools/query.rs`）は取得の間 `cancel` を購読し、割り込みを受けていれば保存しない。`drive`（`src/lib.rs`）はシグナルの後もコマンドを `SHUTDOWN_DRAIN_TIMEOUT` まで走らせて結果を捨てるので、その間に書いたファイルは報告されず、同じパスでの再実行を終了 74 にする。購読が要るのは、`drive` が使う `watch::Sender::send` が受信側の無いときに値を保存しないためである |
| 出力 | `Scout::fetch_image_to` が `path`、`url`、`content_type`、`bytes` の frontmatter ブロックを出し、本文は出さない。`--json` は同じ 4 項目を `data` に持つ。`content_type` は `ImageFormat::mime` の固定文字列で、応答ヘッダの文字列を出力へ通さない。`path` と `url` は `write_yaml_str_whole`（`src/yaml.rs`）でエスケープする。`write_yaml_str` は値を 450 バイトで切るので使わない。切られたパスは実在しない |
| `--output` の無い画像 | `fetch_page` 側の `UnsupportedContentType` のままで、メッセージは変えない。分類の hint だけを `--output` の案内に替える |
| 画像以外への `--output` | `FetchError::NotAnImage`（終了 65）。本文を読む前にヘッダで判定する。`--output` を外す案内を出すのは、`is_fetchable_as_text`（`src/fetch/download.rs`）が真の媒体だけである。SVG や JSON は text の取得も拒否するので、外しても同じ終了 65 になる。Slack permalink は取得前に使用法エラー（終了 64）にする |
| 画像への `--js` | `FetchError::ImageWithJs`（終了 65）。`fetch_image` は `open_image` の後、`ImageResponse::read` の前で判定する。`open_image` より前に置くと、ページの URL に `--js --output` を付けた場合に原因である `NotAnImage` を報告できない。`read` より後に置くと、タイムアウト、サイズ超過、シグネチャの不一致が先に失敗し、`--js` を外す案内が出ない |
| `--raw` | 画像には作用しない。HTML の変換方式を選ぶフラグであり、画像に変換は無い |
| `research` | 変更しない。画像は従来と同じメッセージで `failed_urls` に載る |

画像への `--js` の拒否は、DR-0034 の「明示的な `--js` は媒体にかかわらず既存の強制レンダリング要求を優先する」という文言に対する例外である。静止したラスタ画像にブラウザの描画が加えるものは無いので、要求を黙って読み替えずに失敗させる。基準版でも `fetch_page` は描画の前に `download` で画像を拒否しており、`--output` の無い経路の挙動は変わらない。

### Consequences

- Good, because 画像のバイト列が変換されずにエージェントへ届く。scout は画素を解釈しない。
- Good, because 画像の取得は text と同じ SSRF 検証、リダイレクト上限、サイズ上限を通る。
- Good, because `create_new` が存在確認と作成を 1 回の操作で行うので、確認と書き込みの間に別のファイルが現れても上書きしない。
- Bad, because scout が初めてファイルを書く。終了 74 を作る経路は、CDP の失敗、HTTP client と proxy の構築失敗、標準出力への書き込み失敗（[DR-0002](0002-adopt-sysexitsh-exit-code-convention-for-cli.md) の 2026-11-10 追記）に `--output` の書き込み失敗が加わり 4 つになる。
- Bad, because DR-0034 の `--js` の文言に、媒体による例外が 1 つ入る。
- Bad, because `--js --output` はどの応答でも成功しない組み合わせだが、どちらのフラグを外すべきかは応答を見るまで決まらないので、拒否までに 1 回のリクエストを使う。
- Bad, because 1 回の呼び出しで保存するのは 1 枚で、ページの画像を複数見るには画像ごとに呼ぶ。

### Confirmation

`tests/fetch_image.rs` は mock proxy を使い、SSRF を緩めずに実 CLI の取得から書き出しまでを通す。T-C061 は 4 形式のバイト列の一致と frontmatter、T-C062 は JSON の `data`、T-C063 は `--output` の無い画像、T-C064 はシグネチャの不一致、T-C065 は既存ファイル、T-C066 は親ディレクトリ不在を、proxy を設定せずリクエストの前に拒否することと合わせて確認する。T-C067 は画像以外への `--output` と、`--output` を外す案内が text として取得できる媒体にだけ出ることを確認する。T-C068 は画像への `--js` を、PNG ではない本文でも `--js` を外す案内が出ることと合わせて確認する。T-C069 は、ページへの `--js --output` が `--output` を外す案内を出すことを確認する。T-C070 は 450 バイトを超える URL とパスが切られずに出ること、T-C071 は UTF-8 でない保存先が終了 64 になることを確認する。

`src/fetch/download/download_tests.rs` の T-F012 は、text の取得が画像を従来のメッセージで拒否することを確認する。`research` が `failed_urls` に載せる理由はこのメッセージである。T-F093 は上限を超える画像、T-F094 は画像経路でのリダイレクト先の private IP 遮断を確認する。`src/tools/save/tests.rs` の T-TS043 は書き込み途中の失敗でファイルが残らないこと、T-TS044 は書き込みの時点で既存ファイルを置き換えないこと、T-TS045 は事前確認の判定を確認する。`src/tools/image_fetch_tests.rs` の T-TS046 は、取得中に届いた割り込みが保存を止めることを確認する。`sync_all` の失敗を起こすテストは無い。`src/tools/slack_fetch_tests.rs` の T-SK092 は Slack permalink への `--output`、`src/tools/errors/exit_code_tests.rs` の T-ER001b は新しい 3 つのエラーの終了 65 を確認する。

実装前に `SCOUT_NETWORK_TESTS=1 cargo nextest run --test fetch_image` を実行し、T-C061 から T-C068 の 8 件が失敗することを確かめた。実装後に、シグネチャ照合、Slack の `--output` 判定、書きかけファイルの削除、`create_new` の 4 箇所を壊し、対応するテストが失敗することを確かめた。`--js` の判定は `read` の後と `open_image` の前へそれぞれ動かし、前者で T-C068、後者で T-C069 が失敗することを確かめた。割り込みは、購読を外した場合と判定を外した場合のどちらでも T-TS046 が失敗することを確かめた。T-C065、T-C066、T-C067、T-C070、T-C071 は、対応する修正の前に失敗することを確かめた。

## Pros and Cons of the Options

### 呼び出し側が `--output <path>` で指定したファイルへ保存する

- Good, because 保存先が呼び出し側の読める場所になる。
- Good, because 標準出力はテキストのままで、既存の JSON envelope（[DR-0010](0010-scout-local-json-envelope-contract.md)）に載る。
- Bad, because 呼び出し側は保存先を決め、保存後にファイルを開く 2 段の手順を踏む。

### scout が一時ディレクトリを選んで保存し、そのパスを返す

- Good, because 呼び出しが 1 引数で済む。
- Bad, because scout が選んだ場所を呼び出し側の sandbox が読めるとは限らない。
- Bad, because 残ったファイルを誰が消すかが決まらない。

### MCP サーバとして画像の content block を返す

- Good, because ファイルを介さずに画像入力へ渡せる。
- Bad, because MCP サーバ提供は Non-goals にあり、CLI として直接呼ばれる前提を崩す。

### scout の中で OCR またはキャプション生成を行い、テキストで返す

- Good, because 標準出力だけで完結する。
- Bad, because 届くのは画像の解釈であり、一次ソースではない。要約と統合判断を scout が代替することは Non-goals にある。

### ページ内の全画像を一括で保存する

- Good, because ページの画像を 1 回で揃えられる。
- Bad, because 枚数と合計サイズの上限、画像ごとの SSRF 検証、失敗した画像の報告を新たに決める必要がある。Issue #547 が求めるのは 1 枚を画像入力として渡すことである。

## More Information

合意の出所は Issue #547 の「合意と範囲」節である。保存先を `--output` で指定すること、既存ファイルの拒否、シグネチャ照合、終了コード、`--js` の拒否、出力の形はその節に載っている。経路の分け方、エラー型、判定の順序は実装担当が選び、本記録でレビューへ提示する。

`.claude/OUTCOME.md` の Non-goals にある「ローカルファイル処理」は、ローカルファイルを入力として読むことを指す文へ直す。`.gitignore` が `.claude/` を無視しているので、その修正はこの変更に含まれない。

`--js` の例外は 2026-11-12 に `/challenge` へ通した。判定は Conditional GO で、critic-design の 2 件はどちらも weakened だった。条件は、`--js` の判定を媒体の判定の直後かつ本文の読み込みの前に置くことと、その順序をテストで固定することの 2 つで、どちらも本変更で満たした。`--js` と `--output` を clap の排他指定にして取得前に拒否する案は、終了コードが 64 になり Issue #547 の合意（終了 65）と食い違うので採っていない。

保存先の事前確認、`sync_all`、UTF-8 の制約、割り込み時の扱い、`write_yaml_str_whole`、`NotAnImage` の hint の分岐は、2026-11-12 のコードレビューの指摘を受けて加えた。Content-Type が無い応答や `application/octet-stream` の応答をシグネチャで画像と判定する案は、Issue #547 が合意した受理範囲の外なので採っていない。

### Reassessment Triggers

- ページ内の複数の画像を 1 回の呼び出しで保存する要求が来た場合
- SVG、その他の画像媒体、PDF などバイナリ媒体の保存を求められた場合
- text の応答も `--output` でファイルへ書く要求が来た場合
- MCP サーバ提供が Non-goals から外れた場合
- 画像読み込みツールの上限が 10,000,000 バイトより小さく、保存した画像を開けない事例が出た場合
