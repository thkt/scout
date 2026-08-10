# scout Rust コード評価 — 他リポジトリへの流用視点

対象: scout v2.4.0 (edition 2024, rust-version 1.97.1)
測定日: 2026-08-11/基準 commit 5b2b9cc

他リポジトリを scout の書き方に寄せてリファクタリングするための参照資料として書いた。scout 自身の改善点は E 節と F 節に分けてある。初版 (2026-08-10) は F 節の指摘を 3 commit で修正した時点のもので、2026-08-11 に実装ファイル 50 本を 1 本ずつ読み切って D 節を追加した。

## 実測値

| 指標                                              | 値                                                                                                                                                                                               | 取得方法                                                                                                                                           |
| ------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| Rust ファイル                                     | 99 (`src/` 95 / `tests/` 4)                                                                                                                                                                      | `find -name '*.rs'`                                                                                                                                |
| 総行数                                            | 24,590                                                                                                                                                                                           | `wc -l`                                                                                                                                            |
| src の実装ファイル                                | 50 ファイル / 12,227 行                                                                                                                                                                          | ファイル名が `*tests.rs` / `*_tests.rs` でないもの。19 ファイルが inline `mod tests` を含み、`test_support.rs` と `test_helpers.rs` もこの側に入る |
| src のテスト専用ファイル                          | 45 ファイル / 10,986 行                                                                                                                                                                          | ファイル名が `*tests.rs` / `*_tests.rs` のもの                                                                                                     |
| `tests/` ディレクトリ                             | 4 ファイル / 1,377 行                                                                                                                                                                            | 統合テスト                                                                                                                                         |
| テスト数                                          | 731 passed / 1 skipped                                                                                                                                                                           | `cargo nextest run --all-features`                                                                                                                 |
| lint 抑制                                         | 14。内訳は `#[expect(...)]` 7、`#[cfg_attr(not(feature = "js-rendering"), allow(...))]` 6 (`fetch/cdp/` 配下、feature 無効ビルドでのみ発火)、`#![allow(dead_code)]` 1 (`tests/common/mod.rs:17`) | `grep -E '#!?\[(cfg_attr\(.*)?(allow\|expect)\('`。`#\[allow` だけの grep は cfg_attr 経由の 6 件も inner attribute も拾わない                     |
| 本番経路の `unwrap()` / `expect()`                | 3 (`rng.rs:43` mutex poison、`lib.rs:118` 静的 directive、`envelope.rs:236` infallible Serialize) — 全てに理由コメントあり                                                                       | 全 53 hit のうち、`#[cfg(test)]` 配下と `test_support.rs` / `test_helpers.rs` を除いた残り                                                         |
| clippy `--all-targets --all-features -D warnings` | exit 0                                                                                                                                                                                           | 実行済み                                                                                                                                           |
| clippy deny                                       | 13 (`Cargo.toml` の `[lints.clippy]` セクションのみ。`[lints.rust]` の `unsafe_code` はここに含めない)                                                                                           | —                                                                                                                                                  |
| `unsafe`                                          | `unsafe_code = "forbid"` (`[lints.rust]`)                                                                                                                                                        | —                                                                                                                                                  |
| 到達できない `pub`                                | 0。`unreachable_pub = "deny"`                                                                                                                                                                    | —                                                                                                                                                  |
| テスト ID `[T-XXX]`                               | 出現 658 / 重複除去後 658 (重複定義なし)                                                                                                                                                         | `grep -o` と `sort -u` の両方                                                                                                                      |
| 実装コード中の DR/ADR 参照                        | 131 箇所                                                                                                                                                                                         | `grep -o 'ADR-[0-9]\{4\}\|DR-[0-9]\{4\}'`                                                                                                          |
| Decision Record                                   | 25 件 (全て accepted)                                                                                                                                                                            | `docs/decisions/`                                                                                                                                  |

13 個の clippy deny を宣言し、抑制 14 個で通っている。うち 6 個は `js-rendering` 無効ビルドで未使用になるコードを黙らせる `cfg_attr` 経由で、feature gate の副作用を消す定型。残る 8 個のうち 7 個は `#[expect]` で、抑制が不要になった日にそれ自体が警告として報告される形になっている。deny リストを `#[allow]` で骨抜きにしているリポジトリとは別物。

---

## A. そのまま移植できる — 言語非依存の運用

対象リポジトリが Rust でなくても効く。

### A-1. エラー分類 → 終了コードの一元テーブル

`envelope.rs:176-211` が `ErrorCode` enum を 1 つ持ち、`exit_code()` と `is_retryable()` の両方をそこから導出する。`ScoutError::new` は `retryable` を引数で受けず `kind.is_retryable()` で決める (`tools/errors.rs:40-48`)。呼び出し側が `retryable: true` を書ける余地が無いので、JSON 契約と終了コードが構造的にずれない。

`classify.rs:63-73` の `from_http_status` は「3 つのバックエンドが HTTP status から独自に導出していて、実際に食い違っていた (GitHub 408 が DataError、Brave 404 が DataError)」という履歴をコメントに残したうえで 1 つのテーブルに統合している。逸脱したいバックエンドは委譲アームの手前に自分のアームを足す形で、逸脱が diff に見える。

シグナル由来の終了コードは別マッピングになっている。`signals.rs:16-22` の `InterruptSignal::exit_code()` が SIGINT → 130/SIGTERM → 143 を返す (POSIX の 128 + シグナル番号)。エラー分類とは由来が違うので統合していない。移植時もこの分離は保つ。

移植コスト: 小。enum + 2 メソッド。

### A-2. リトライ / バックオフの一元化 (DR-0006)

`retry.rs` (165 行) が全バックエンドのリトライ判断を持つ。

- `retry_with` は「初回 1 回 + `max_retries` 回」という数え方をユーザー向け契約 (`SCOUT_MAX_RETRIES=N` は「追加 N 回」) にコメントで結び付けている。最後の試行はループの外に出して、ループから抜ける経路がエラーを捏造しなくて済む形にしてある
- `parse_retry_after` が RFC 9110 §10.2.4 の 2 形式 (整数秒/HTTP-date) を扱い、HTTP-date は `Clock` 経由で「今から何秒」に変換する。単位が揃う
- `retry_after_within_cap` が「サーバ指定の待ち時間が上限 (300 秒) を超えるなら、待って再試行しても同じ結果になるので即座に失敗させる」という判断を 1 箇所に置く
- 指数バックオフにも同じ 300 秒上限をかける。理由 (issue #185: `SCOUT_MAX_RETRIES` を上げると `2^attempt` が単発で数分の sleep を生み、外側のツールタイムアウトを超える) がコメントにある
- `is_transient_decode` が reqwest の `is_decode()` を source chain を辿って「転送起因 (リトライ可)」と「スキーマ不一致 (終端)」に分ける。ブール 1 個では区別できないという実測 (issue #113) が根拠

移植コスト: 小。ただし `Clock`/`Rng` の注入 (B-1) が前提。ここが揃わないとバックオフのテストが時間依存になる。

### A-3. `[T-XXX]` テスト ID と DR の相互 pin

テストの doc comment 先頭に `[T-<PREFIX><NNN>]` を書き、DR がその ID を引用してテストを名指しする。規約自体が `test_support.rs:1-19` に書かれている。ブラケット付きは定義、ブラケット無しは引用、という区別まで決めてある (grep で定義が一意に引ける)。

これで「この決定を守っているテストはどれか」が双方向に辿れる。658 ID/131 参照が実際に動いている。重複採番はリポジトリ自身のテスト (`test_support::scan_test_id_violations`) が検出するので、採番ミスは CI で止まる。

移植コスト: 小 (規約のみ)。ただし ID の採番と prefix 設計を最初に決めないと後から一括採番が要る。

### A-4. CI ゲートの構成

`.github/workflows/ci.yml`:

- `cargo check` を通常/feature 有効の 2 通り
- `cargo nextest run` を通常/`--features js-rendering --run-ignored all` の 2 通り。後者で `#[ignore]` を全部走らせるので、chromium 不在のランナーは skip でなく fail する
- clippy を `--all-targets` と `--all-targets --all-features` の 2 通り、いずれも `-D warnings`
- `cargo fmt -- --check`
- PR のみ diff-cover を `--fail-under=95` で。除外は `fetch/cdp/proxy/transport.rs` 1 つだけで、除外理由 (実ソケット障害でしか通らないエラーアーム) が yml のコメントに書いてある
- security job: `cargo deny`/`cargo audit`/`cargo machete --with-metadata`
- 全 action が SHA pin。`persist-credentials: false`。zizmor による workflow 自体の lint

とくに効いているのが `env: SCOUT_NETWORK_TESTS: "1"` と、その理由コメント。loopback bind に失敗したテストはローカルでは skip するが CI では panic する。nextest は成功テストの stderr を隠すので、skip したまま緑になる事故をこれで塞いでいる。

移植コスト: 中。カバレッジゲートを絶対値でなく diff で取る点は他言語でも同じツール系 (diff-cover) が使える。

### A-5. 秘密情報の型による封じ込め

`redacted.rs` の `Redacted(String)`:

- `Debug` を手書きして `[REDACTED]` を出す
- `new()` が空白のみ/空文字で `None` を返す。実質未設定の資格情報を保持できない
- `from_env_var` が「未設定」と「空白のみ」を同じエラーに畳む。エラーからどちらか判別できない

`ssrf.rs:86-120` は URL の userinfo をログ出力時に落とす `RedactedLogUrl<'a>` を持ち、パース失敗時は `[redacted-url]` に倒す (fail closed)。

移植コスト: 小。型を 1 つ作るだけ。

### A-6. `--help` を AI エージェント向けの契約書として書く

`lib.rs:65-94` の `after_help` に終了コード表、環境変数、チューニング用 env var の範囲まで全部載せる。さらにその内容をテストが pin している (`root_help_contains_exit_codes_and_environment`, `root_help_lists_scout_tuning_env_vars`)。`--version` 実行時に「coding agent なら `--help` を読め」というヒントを stderr に出す (`AGENT_HELP_HINT`)。

`--help` に載せた使用例は、テストが実際に clap のパーサへ通して検証する。例だけが仕様から取り残される経路を塞いでいる。

移植コスト: 小。ただしヘルプ本文をテストで pin する運用は文言変更のたびにテストが落ちる。scout は「落ちてよい」と判断している。

---

## B. Rust 固有のイディオム — 移植先が Rust なら

### B-1. `Arc<dyn Trait>` の注入 seam (DR-0008 / DR-0009)

非決定要素をそれぞれ最小トレイトに切って `Arc<dyn _>` で持つ。

| trait         | 本番実装           | テスト実装                                 | ファイル                   |
| ------------- | ------------------ | ------------------------------------------ | -------------------------- |
| `Clock`       | `SystemClock`      | `FixedClock(u64)`                          | `clock.rs` (51 行)         |
| `Rng`         | `FastrandRng`      | `SeededRng`                                | `rng.rs` (80 行)           |
| `TokenSource` | `GhCliSource`      | `StaticTokenSource`                        | `token_source.rs` (126 行) |
| `DnsResolver` | `TokioDnsResolver` | `StaticDnsResolver` / `FailingDnsResolver` | `fetch/ssrf.rs`            |

いずれも 1 メソッド。`Send + Sync` を trait 境界に書いて `Arc<dyn _>` を async タスク間で共有できるようにしている。`DnsResolver` は async メソッドを `Pin<Box<dyn Future>>` の型エイリアス (`DnsLookupFuture`) で返してオブジェクトセーフを保っている — `async fn` in trait をそのまま使うと `dyn` に載らないため。

trait を切るほどでもない副作用は、関数を引数で受ける形にする。`token_source.rs` の `resolve_from_env_or_gh<F, C, Fut>(env_reader, run_gh)` は本番が `spawn_gh` を渡し、テストは subprocess を起動せずに `gh` 経路の分岐を全部通す。trait 1 個 + 実装 2 個より軽い。

移植コスト: 小。ファイル 4 つで 300 行未満。

### B-2. 検証済みを型で表す `ValidatedUrl`

`ssrf.rs:122-156`。`ValidatedUrl` の唯一のコンストラクタが `ssrf_check` で、下流 (`download`, `reqwest::Client::get`) は `&ValidatedUrl` しか受け取らない。SSRF チェックを飛ばす経路がコンパイル時に存在しない。

リダイレクト追跡用の `join()` は生の `url::Url` を返すので、呼び出し側は必ず `ssrf_check` に通し直す。

移植コスト: 小。型システムのある言語なら形は移せる (TypeScript の branded type など)。

### B-3. 遅延初期化した外部クライアント

`Scout.github`/`Scout.slack` が `tokio::sync::OnceCell`。GitHub を使わないコマンドは `gh auth token` のサブプロセス起動を払わない。Slack を使わないコマンドは `SLACK_TOKEN` を読まない。fallible な `slack` 側は `get_or_try_init` を使う。

遅延させたコストは、引数の静的検証より後ろに置く (D-5)。

移植コスト: 小。

### B-4. 環境変数を関数注入で読む

`RuntimeConfig::from_env_with<F: Fn(&str) -> Result<String, VarError>>` (`tools/config.rs:78`)。テストが `unsafe { env::set_var }` を使わずに parse 失敗と範囲外を全部通せる。`unsafe_code = "forbid"` と両立させるための設計だが、env をプロセスグローバル状態として触らないので並列テストが壊れないという利点が本体。

`detect_egress_mode(env: &HashMap<String, String>)` も同じ形 (データを受けてデータを返す)。

`VarError::NotUnicode` を「未設定」でなく「設定済みだが不正」として扱う判断が `read_env_raw:143-154` にある。設定したつもりの値が黙ってデフォルトに落ちない。

移植コスト: 小。

### B-5. degraded (部分失敗) の表現

`envelope.rs:62-89` の `Degradation` はフィールドを private にし、`push(message, reason)` だけを公開する。`notes[i]` と `reasons[i]` の対応がずれた状態を構築できない。`CommandOutput` も `degraded: false` と非空 `notes` の組み合わせを作れない。

`DegradedReason` は enum で JSON に `SCREAMING_SNAKE_CASE` で出る。呼び出し側は自由文の notes をパースせずに失敗モードを判別できる。

型で守れるのは「envelope の中で対応がずれないこと」までで、「通知が利用者に届くこと」は別問題になる (D-4)。

移植コスト: 中。CLI/API の出力契約に関わるので、既存の出力形式があると移行が要る。

### B-6. 最小可視性をコンパイラに答えさせる手順

F-2 の修正で使った手順そのもの。可視性キーワードを人が推測して書くと、必要な範囲より広い値が残り、それが「不要な `pub`」として蓄積する。代わりにコンパイラを判定器として使う。

1. 対象の可視性キーワードを持つ行を全部列挙する
2. **1 行ずつ独立に**最も狭い候補へ変え、コンパイルし、元に戻す。通った行を候補として記録する
3. 候補を全部まとめて適用し、もう一度コンパイルする。壊れたら 1 件ずつ戻して通るまで縮める
4. 残った行に 1 段階広い候補 (`pub(super)`) を当て、2-3 を繰り返す

1 度も通らなかった行が `pub(crate)` に残る。逆に、最も狭い候補で通った行は、その可視性がもともと不要だった証拠になる。scout ではこの手順で、呼び出し元が定義ファイルの外に無い `pub` が 2 件 (`SlackClient::new`、`try_spawn_with_bind`)、定義モジュールの外に無い `pub(crate)` が 21 件見つかった。

一括で変更してエラーから逆引きする形は 2 度試して 2 度とも収束しなかった。1 箇所ずつ独立に試し、その後まとめて適用して壊れた分を戻す形にすると収束する。scout の 301 箇所で 429 秒 (約 460 回のコンパイル)。

注意点が 5 つある。

- **終了コードだけを判定に使わない。** `cargo check` は警告があっても 0 を返す。scout ではこれで `private_interfaces` 警告 6 件を見逃した。`pub(crate)` な enum の variant が `pub(super)` な型を持つと出る警告で、`clippy -D warnings` まで走らせて初めて出た。判定器には lint を含めたコマンドを使う
- **1 箇所ずつ通っても、まとめると壊れる組み合わせがある。**個別判定の後に一括適用して再コンパイルし、壊れた分を 1 件ずつ戻す段階を必ず入れる
- コンパイラのエラーは連鎖する。1 つの型が private になると型推論が崩れ、後続のエラーが出力から消える。1 回のコンパイル結果を全体と読まない
- 構造体リテラルやパターンのフィールド名は定義と同じ見た目をしている。`sed` で行頭パターンだけを対象にしないと、`Foo { bar: 1 }` の `bar` にも可視性キーワードが挿入されてパースエラーになる。BSD sed は `\b` を解釈しないので、単語境界を当てにした置換は黙って空振りする
- 「証明された最小」と「リポジトリで統一された 1 種類」は別の基準。Rust には private/`pub(super)`/`pub(in path)`/`pub(crate)`/`pub` の 5 段階があり、最小を追うと 3 種類以上が混ざる。混ぜるか揃えるかを先に決める

移植コスト: 小。可視性の段階を持つ言語 (Rust、Java、C#、Kotlin) ならそのまま使える。TypeScript のように `private`/`public` の 2 段階しかない言語では、ファイル境界と export の有無が同じ役割を果たす。

---

## C. 形は参考にする、機構はそのまま持ち込まない

### C-1. production 型に載る `#[cfg(test)]` フィールド

`ScoutBuilder` は `github_endpoint`/`slack_endpoint` を `#[cfg(test)]` フィールドとして持ち、`build()` の中で `#[cfg(test)]` ブロックが `OnceCell` を事前に埋める (`tools/builder.rs:52-59, 253-271`)。DR-0008 で意図的に選んだ設計で、`with_*` セッターも全て `#[cfg(test)]`。

scout では成立している (本番ビルドに一切残らない、seam の全経路が DR で説明されている)。ただし、テスト用の分岐を本番型の定義に持ち込む形なので、DR 相当の説明を書かないまま他リポジトリに真似すると「テストのためだけの本番コード」になる。移すなら「注入 seam を builder に集約する」という形だけ取り、endpoint 差し替えは別の手段 (設定値として本番にも存在させる) を検討する。

### C-2. コメントの密度

`tools/config.rs:13-25` の `DEFAULT_GITHUB_TIMEOUT_SECS` は 12 行のコメントで、180 秒という値が「最も重い repo-overview の happy path を通す」「全タイムアウト時のリトライ予算 279 秒を下回る」という 2 つの境界から決まったことと、そのトレードオフを書いている。`classify.rs:52-62` も同様に「なぜ 1 箇所なのか」を過去の食い違い実例で説明する。

これは scout の強みだが、コメントを書ける人が値の由来を実測している前提で成立する。密度だけ真似ると「コードを言い換えただけの長文コメント」が増える。移すなら「値の由来と却下した代替を書く」という基準だけを移す。

### C-3. テストファイルの分割粒度

テスト専用ファイルが 45 個。`fetch/cdp/` は `launch.rs` + `launch/` 配下 4 テストファイル、`proxy.rs` + `proxy/` 配下 2 ファイルという構成。関心ごとに分けてあるので 1 ファイルは読み切れるが、ファイル数は実装の 1 対 1 を超える。

小規模リポジトリにそのまま持ち込むとディレクトリが薄いファイルで埋まる。分割の閾値 (scout は 300-400 行あたり) を先に決めてから移す。

---

## D. 欠陥の型 — 他リポジトリでも同じ探し方が使える

実装ファイル 50 本を 1 本ずつ読んで 29 commit を積んだ結果、見つかった欠陥は 5 つの型に収まった。型ごとに「どう探すか」を書く。個別の修正内容より、この探し方のほうが移植価値が高い。

### D-1. 文書化された保証を誰も支えていない

コメントや ADR が「こう振る舞う」と書いているのに、その主張を壊してもテストが 1 本も落ちない状態。scout で見つかった実例。

| 主張                                                                                                  | 壊しても緑だったもの                                           |
| ----------------------------------------------------------------------------------------------------- | -------------------------------------------------------------- |
| `charset.rs` が 8 エンコーディングを許可する                                                          | 8 つのうち 3 つはテストが無く、削除しても 688 テストが緑のまま |
| `token_source` は `gh` の stderr を握り潰す (資格情報が混じるため)                                    | 「診断のため stderr を出す」変更が無害に見える状態だった       |
| `SlackError::Server` は ADR-0003 の共通表からわざと外れる                                             | 「他と同じく `from_http_status` に委譲する」変更が通った       |
| `From<reqwest::Error>` が `without_url()` でエラー文から URL を落とす (query string にトークンが載る) | 落とさなくても緑                                               |
| `ContentsPayload` の doc がコードと逆のことを書いている                                               | そもそもコードが doc と違った                                  |
| DR-0018 が「このテストは `gh` subprocess に到達する」と書く                                           | 到達していなかった                                             |

探し方: doc/ADR の主張文を 1 つずつ取り出し、**その主張を実際に壊してテストを走らせる**。落ちなければテストを書く。読んで探すのではなく壊して探す。落ちなかったこと自体が発見になる。

### D-2. ASCII 固定の fixture が単位と境界の欠陥を隠す

閾値をバイトで数えているか文字で数えているかは、ASCII だけの fixture では区別がつかない。

| 箇所                                                        | 隠れていたもの                                                                                 |
| ----------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| `truncate_with_note`                                        | 多バイト文字の途中で切るテストが無かった                                                       |
| `has_thin_body` (JS レンダリング要否の判定)                 | バイトで数えていたため、日本語の SPA は英語ページの 1/3 の分量で「本文あり」の判定を超えていた |
| `apply_line_range` (`github/helpers.rs:165`) の行番号カラム | 幅 5 固定で、10 万行 (10MB 上限に対して約 1MB) から桁が溢れる                                  |

探し方: 閾値を持つ処理を列挙し、それぞれ「バイトか文字か」を実装で確認する。fixture に非 ASCII を 1 文字入れる。上限値そのものではなく、上限に到達しうる現実的な入力サイズを見積もる。

### D-3. 成功経路の上限を error 経路が迂回する

Brave と GitHub は成功レスポンスを `read_body_capped` (1MiB/10MB) で読む一方、エラー時の診断メッセージを `Response::text()` で組み立てていた。上限が「本文が有用な経路」にだけ適用されていた。

探し方: 上限を持つ処理の隣で、同じリソースを読む別経路 (エラー、ログ、診断、リトライ) を探す。

この型には注意点がある。scout では上限を守っていることを**出力から検証できなかった**。Brave は診断文をさらに `BODY_SNIPPET_BYTES` で、GitHub は `extract_error_message` の 200 文字で切るので、64KiB 読んでも 20MB 読んでも出力が同一になる。判別するには EOF を返さない生 TcpListener が要り、失敗の形が timeout になって CI で不安定になる。

そこで振る舞いテストではなく lint で守った。`clippy.toml` の `disallowed-methods` で `reqwest::Response::{text, bytes, json}` を deny し、reason に代替 (`read_body_capped`/`read_body_snippet`) を書く。呼び出しを戻した瞬間にコンパイルが止まり、「テストは全部緑のまま回帰する」経路が消える。**上限が出力に現れないなら、テストではなく lint で守る。**

### D-4. 機械向けチャネルにだけ出す劣化通知

`scout fetch` は文字コードを確定できなかったとき `DecodeUncertain` を JSON envelope の `degraded_reasons` に積むが、markdown 本文には何も出していなかった。`--json` 無しの実行では envelope が出力されないので、既定モードの利用者には警告が 1 文字も届かない。同じページを `research` 経由で取ると各ページに注記が付く。通知文自体が「Do not trust it as a faithful primary source」と書いており、届くことが前提の警告だった。

探し方: 出力チャネルが 2 つ以上あるなら (人間向け/機械向け、stdout/stderr、markdown/JSON)、劣化通知が全チャネルに出るかを 1 件ずつ確認する。同じ劣化を扱う別コマンドがあるなら、そちらとの差分を見る。

同種の問題として、通知の書式が経路ごとに違う場合もある。scout は `> Note: ` を 4 箇所で使いながら、`repo-overview` だけ `> **Note:** ` を出していた。呼び出し側が 2 形を照合する必要があった。

### D-5. 課金 resource の前に静的検証を置く

`repo-tree` は GitHub client を構築した後で `--path` を検証していた。client の初回構築は `gh auth token` の subprocess 起動を伴う (実測でテスト 1 ケースあたり約 1 秒)。`--path` と `--ref` はどちらも引数の文字列の形だけを見る検証で、ネットワークも subprocess も必要としない。ハンドラ自身のコメントは「静的な棄却を先に行う」と書いていた。

探し方: ハンドラの先頭から順に、各行が「引数の形だけで判定できるか」「外部 resource を消費するか」で分類する。前者が後者より後ろにあれば入れ替える。同じ種類の別ハンドラと並べて読むと差が見える (scout では `repo_read` が正しい順序だった)。

### D-6. 検証手順

上の 5 型を直すとき、修正が本当に効いているかの確認に使った手順。

- **回帰を注入して新テストが落ちることを確認してから戻す。**テストを書いて緑になっただけでは、テストが何も見ていない可能性が残る。scout では書いた回帰テストを毎回この方法で確認した
- **`git checkout -- <file>` を注入の巻き戻しに使わない。**自分がまだ commit していない実装ごと消える。scout では 2 回消した。文字列置換で保存/復元する
- **ツールが答える範囲を名指しする。** `cargo machete` が答えるのは「未使用の依存」であって「不要な依存」ではない。ツールの出力が画面上の唯一の情報なので、答えの全体に見えてしまう
- **数を文書に書く前に、grep の pattern が覆う範囲と文中の名詞を突き合わせる。** `#[allow]` を数える pattern は `#![allow]` にも `#[expect]` にもマッチしない。「抑制」という名詞はその 3 つ全部を指す
- **最適化の前に測る。** `closest_matches` は 10 万パスに対して 194-715ms で、5 秒の予算に対して枝刈りは不要だった。setext heading の対応は、star 上位 100 リポジトリの README で 5 件が setext を使っていた (linux と 996.ICU はほぼ全部 setext) ことを測ってから実装した

---

## E. scout 側の改善ポイント — 未着手

### E-1. `surface_overrides` の 5 連 if — 現状維持

`tools/config.rs:105-136` が 5 フィールド分ほぼ同形の if を並べる。フィールド名が `tracing` の構造化ログのキーになっているので、ループで畳むとキーが文字列になり、`info!` のフィールド名の静的性が失われる。マクロで畳めるが、5 個のためにマクロを増やすのは割に合わない。

判断: 現状維持でよい。フィールドが 8-10 個に増えたら宣言的マクロを検討する境界。

### E-2. `with_clock` / `with_rng` の 4 重複 — 決着済み

`GitHubClient`/`BraveClient`/`SlackClient`/`ScoutBuilder` の 4 箇所に同形の `with_clock`/`with_rng` がある (計 8 メソッド)。共通化 (ClientCommon 化、DRY-02) は critic-design の実測で棄却済み。ADR-0007 の Reassessment Trigger が根拠にならない (ADR-0004 が ADR-0007 に先行する) と実証されたため、新 DR の起草が着手条件として #310 の Backlog に残っている。

他リポジトリへ移すときは、この重複を「直っていない」と読まないこと。3 クライアントが同じ trait を実装していないのは、共有部分を切り出すコストが利得を上回るという判断の結果。

### E-3. `api_get_once` の二重パース — 現状維持

`slack/client.rs:250-271` は本文を `serde_json::Value` へ 1 回、`from_value` で目的の型へもう 1 回パースする。`ok: false` を目的型の deserialize より先に判定するために必要な形で、`#[serde(flatten)]` で 1 回に畳むと、`ok: false` かつ目的型に合わない本文が `Api` ではなく `Decode` に落ちる (エラー分類が変わる)。1 レスポンスは `MAX_API_RESPONSE_BYTES` (1MiB) で上限があるので、得るものより失うもののほうが大きい。

---

## F. 解消済みの改善ポイント (2026-08-10 の 3 commit)

### F-1. コメント言語の混在 — 3381fec で解消

21 ファイル 121 行に日本語コメントがあり、残りは英語だった。内訳はテスト専用ファイルが 97 行、実装ファイルが 24 行。混在の仕方が 2 種類あった。

- 段落単位の日本語: `redacted.rs` のテスト doc comment (`[T-RD006] 未設定の env 変数は注入された欠落エラーになる`)
- 英文の中に日本語 1 語: `classify.rs:69,96` の `// 退避: 1xx/3xx reaching an error path is not...`

全 121 行を英語へ揃えた。`退避` は ADR-0011 が Unknown スロットに与えた設計語彙で、`envelope.rs:202` が既に `retreat slot` と英語で書いていたため、そちらへ寄せて 8 箇所を統一した。

7 行だけ日本語が残る。charset テストが encode している Shift_JIS/EUC-JP 文字列の引用 (`// "テスト" in Shift_JIS`) で、英訳するとバイト配列が何を表すか読めなくなる。

### F-2. 可視性が揃っていない — 76f8907 / 7fca4e5 で解消

`pub(crate) struct Degradation` の中に `pub fn push` と `pub(crate) fn` が混在するなど、`pub` と `pub(crate)` の使い分けに意図があるように見えて実際は無い状態だった。到達できない `pub` が 58 箇所。

一括で `pub(crate)` に置換する代わりに、コンパイラに最小可視性を答えさせた (手順は B-6)。結果は 84 箇所が `pub(crate)`、53 箇所が `pub(super)`、2 箇所 (`SlackClient::new`、`try_spawn_with_bind`) が定義ファイルの外に呼び出し元を持たず private。フィールド 80 箇所も同じ手順で絞った。

再発防止に `unreachable_pub = "deny"` を追加した。この lint はアイテムだけを見るのでフィールドは守らない。

7fca4e5 で既存の `pub(crate)` 301 箇所も同じ基準で測り直した。21 箇所が定義モジュールの外に呼び出し元を持たず private になった。最終分布は `pub(crate)` 241/`pub(super)` 134/private 21。可視性が 3 段階混在するが、それが各アイテムの実際の到達範囲。

---

## G. 移植の優先順位

コストは移植先の規模に依存する。以下は 1 万行規模の CLI/サービスを想定。

| 順  | 項目                                                    | 節  | コスト                                  | 効き方                                      |
| --- | ------------------------------------------------------- | --- | --------------------------------------- | ------------------------------------------- |
| 1   | エラー分類 → 終了コード / retryable の一元テーブル      | A-1 | 小                                      | 契約のずれが構造的に起きなくなる            |
| 2   | CI ゲート (fmt / lint deny / diff coverage / dep audit) | A-4 | 中                                      | 以降の全変更に効く。最初に入れる            |
| 3   | 秘密情報の型封じ込め                                    | A-5 | 小                                      | ログ流出の経路が型で塞がる                  |
| 4   | 非決定要素の trait 注入 (Clock / Rng / Token / DNS)     | B-1 | 小                                      | テストが時間と乱数から独立する              |
| 5   | リトライ / バックオフ / Retry-After の一元化            | A-2 | 小 (B-1 が前提)                         | 上限とリトライ回数の数え方が 1 箇所に集まる |
| 6   | 検証済みを型で表す (ValidatedUrl 型)                    | B-2 | 小                                      | 検証を飛ばす経路が消える                    |
| 7   | `[T-XXX]` ID と DR の相互 pin                           | A-3 | 小 (規約) / 中 (既存テストへの遡及採番) | 決定とテストが双方向に辿れる                |
| 8   | env を関数注入で読む                                    | B-4 | 小                                      | 並列テストがプロセス env を壊さない         |
| 9   | degraded の型による表現                                 | B-5 | 中                                      | 出力契約の変更を伴う                        |

既存コードの棚卸しに入る前に D 節を読む。D-1 から D-5 は探索の順序としてもこの並びで使える (主張を壊す → fixture に非 ASCII を入れる → エラー経路を追う → チャネル差分を見る → ハンドラ先頭の順序を見る)。

clippy の 13 deny リスト (`Cargo.toml` の `[lints.clippy]`) は 2.4 万行の CLI では快適に通るが、大規模アプリに一括で入れると既存コードが大量に落ちる。`needless_pass_by_value` と `cast_possible_truncation` はとくに影響範囲が広い。段階的に 1 つずつ deny に上げる形が現実的。
