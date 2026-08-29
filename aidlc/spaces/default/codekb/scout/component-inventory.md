# Component Inventory — scout

コンポーネントは 7 つ。名前は `src/lib.rs` の `mod` 宣言に対応するモジュール名をそのまま使う。**この見出し語は `reverse-engineering-timestamp.md` の `analyzed.components` と文字単位で一致させてある。** rerun guard がこの 2 つをリテラル比較するため、見出しを言い換えると次回の再走査がこの走査のカバレッジを引き当てられなくなる。

各節は 5 項目で構成する。

| 項目         | 意味                                                                                                                                      |
| ------------ | ----------------------------------------------------------------------------------------------------------------------------------------- |
| 責務         | このコンポーネントが 1 つだけ持つ仕事                                                                                                     |
| 主なファイル | 実装の所在                                                                                                                                |
| 依存         | crate 内の依存先。crate 外の依存は `dependencies.md` が持つ                                                                               |
| 健全性       | healthy (このコンポーネントに固有の未着手の判断が無い) / at-risk (再着手の条件だけが残り着手が未定の判断を抱える) / degraded (対処が必要) |
| 確度         | この記述の裏付け。full-read (全行を読んだ) / grep-verified (コマンドで主張だけ検証した) / unread (未読、監査文書や索引の引き写し)         |

依存の向きは `main` から横断リーフへの一方向で、循環は無い。全体図は `architecture.md` の `## コンポーネント関係` にある。

**`with_clock` / `with_rng` の 4 重複は 4 コンポーネント (`tools`、`github`、`slack`、`brave`) に跨るため、個々の健全性には算入しない。** 共通化は実測のうえ棄却済みで、再着手の条件は「新 DR の起草」である。その条件が closed issue #310 の中にしか残っていないという追跡上の問題は、コンポーネント単位ではなくリポジトリ単位の項目として `code-quality-assessment.md` の `## 技術的負債` が持つ。

**健全性が attempt 2 で 2 つ動いた。** `tools` と `slack` は監査文書 E-1/E-3 が残していた判断が決着し、どちらも「現状維持で正しい」と確定したので at-risk から healthy へ移した。`fetch` は E-4 の切り出し判断が依然として未着手なので at-risk のまま残る。

## tools

**責務**: CLI ハンドラ層。`Command` の 6 分岐ディスパッチ、stdin フォールバック、`Scout` の依存注入、`ScoutError` の集約。

**主なファイル**: `src/tools.rs` (333)、`src/tools/params.rs` (540)、`src/tools/query.rs` (329)、`src/tools/repo.rs` (548)、`src/tools/builder.rs` (303)、`src/tools/config.rs` (477)、`src/tools/errors.rs` (189)、`src/tools/typo.rs` (146)、`src/tools/test_helpers.rs` (60)

**依存**: `brave::client`、`clock`、`envelope`、`fetch`、`github`、`markdown`、`rng`、`slack`、`token_source`、`yaml`。逆向きの import は持たない。

**健全性**: healthy。**at-risk から移した。** 監査文書 E-1 が残していた `surface_overrides` の再検討閾値 (「フィールドが 8-10 個」) は、attempt 2 が 3 通りの数え方で測ってすべて 5 と確定し、現状維持が正しいことが裏付いた (`code-quality-assessment.md` の `### E-1`)。このコンポーネントに固有の未着手の判断はもう無い。`src/tools/builder.rs` は 4 重複の 1 つを持つが、これは上の前置きのとおり健全性に算入しない。

**確度**: `src/tools.rs`、`src/tools/test_helpers.rs`、`src/tools/config.rs` は full-read。`Scout` 構造体の 12 フィールド、`OnceCell` による GitHub/Slack の遅延初期化、`StdinResolver` の 3 状態、`with_github_timeout` の外側タイムアウトは前 2 者の読みによる。`config.rs` からは `RuntimeConfig` の 5 フィールド、`ENV_*` 定数 5 本、`from_env_with` による env 読み取りの注入、`read_env_raw` の `VarError::NotUnicode` 分岐、`TIMEOUT_MIN_SECS`/`TIMEOUT_MAX_SECS`/`RETRIES_CAP` の範囲、および `T-CFG021`/`T-CFG025`/`T-CFG026` が値として押さえるタイムアウト階層が確定した。残る 6 ファイルは unread または部分読みで、`enum Command` の 6 variant と `DEFAULT_*_SECS` は grep-verified。

## fetch

**責務**: URL 検証 → DNS 事前検査 → ダウンロード → redirect ごとの再検査 → 本文抽出 → Markdown 変換。SSRF 防御はこのパイプラインの骨格そのものである。

**主なファイル**: `src/fetch.rs` (462)、`src/fetch/ssrf.rs` (370)、`src/fetch/download.rs` (243)、`src/fetch/extractor.rs` (433)、`src/fetch/converter.rs` (3,131)、`src/fetch/cdp.rs` (334)、`src/fetch/cdp/launch.rs` (289)、`src/fetch/cdp/proxy.rs` (182)、`src/fetch/cdp/proxy/transport.rs` (114)

**依存**: 横断リーフ (`classify`、`envelope`、`retry`、`body_limit`、`markdown`、`charset`)。`js-rendering` feature が有効なときだけ `chromiumoxide`/`nix`/`tempfile` を使う。

**健全性**: at-risk。`src/fetch/converter.rs` の 3,131 行がこのリポジトリ自身の分割規約から外れる唯一の実装ファイルで、**切り出す単位の判断は依然として未着手である** (監査文書 E-4)。attempt 2 で判断の材料は揃った — そのまま出せる連続区間が 3 つ、並べ替えが要る関心が 2 つ、テスト分割と実装分割のコスト差が測ってある (`code-quality-assessment.md` の `### E-4`)。防御面の負債は無い — CDP 経路の SSRF 非対称は issue #201 (2026-06-16 close) で解消済みである。

**確度**: `src/fetch.rs`、`src/fetch/cdp/launch.rs`、`src/fetch/cdp/proxy/transport.rs`、`src/fetch/converter.rs` は full-read。`FetchError` 14 variant と `classify()` のアーム順、`is_js_dependent`/`has_thin_body`/`is_thin_extract` の閾値 3 種、chromium バイナリ探索テーブル、`build_launch_args` の 12 フラグ、`check_browser_request` の scheme 分岐、`spawn_chromium_pgroup` の pgroup 取得、`parse_ws_url_from_lines`、`reap_pgroup` の SIGTERM → 50ms → SIGKILL、SOCKS5 の accept ループと `UPSTREAM_DIAL_TIMEOUT` 10 秒/`ACCEPT_RETRY_BACKOFF` 50ms/`dial_and_tunnel` は前 3 者の読みによる。`converter.rs` からは実装 1-985 行の可視性分布 (素の private `fn` 38 個、外へ出ている項目 10 個)、6 個の共有ヘルパー、テスト 79 本の関心割り当てが確定した。`ssrf.rs`、`download.rs`、`extractor.rs`、`cdp.rs` は unread。

## github

**責務**: GitHub REST v3 クライアント。パスと ref の検証、base64 とエンコーディングの復号、Markdown 整形、ワイヤ型の定義。

**主なファイル**: `src/github.rs` (411)、`src/github/helpers.rs` (209)、`src/github/encoding.rs` (176)、`src/github/errors.rs`、`src/github/format.rs` (284)、`src/github/types.rs` (251)

**依存**: 横断リーフ (`classify`、`retry`、`body_limit`、`redacted`、`token_source`、`charset`、`markdown`)。

**健全性**: healthy。このコンポーネントに固有の未着手の判断は無い。4 重複の 1 つを持つが、これは上の前置きのとおり健全性に算入しない。

**確度**: `src/github/types.rs` は full-read。ワイヤ型 13 個 (struct 11 + enum 2 `EntryType`/`ContentsPayload`)、`null_as_empty_vec`、`ContentsPayload` の `untagged` 2 arm と `Vec<IgnoredAny>` の設計理由、`real_issues` フィルタはこの読みによる。`src/github.rs` 本体は unread で、エンドポイント 8 本と `API_BASE` は grep-verified。**この節の記述をエンドポイント / メソッドの粒度より細かくしないこと。**

## slack

**責務**: Slack Web API クライアント。permalink の解析、mention の人名置換、YAML frontmatter 付き出力。

**主なファイル**: `src/slack.rs` (193)、`src/slack/client.rs` (620)、`src/slack/url.rs`、`src/slack/mention.rs`、`src/slack/format.rs`

**依存**: 横断リーフ (`classify`、`retry`、`body_limit`、`redacted`、`yaml`、`markdown`)。`users.info` の並列解決に `futures` の `stream::buffer_unordered` を使う。

**健全性**: healthy。**at-risk から移した。** 監査文書 E-3 が残していた `api_get_once` の二重パースは、attempt 2 が `src/slack/client.rs` を開いて決着させた — 現状維持は正しく、棄却の根拠は監査文書の論拠より強いものがコードにある (`code-quality-assessment.md` の `### E-3`)。このコンポーネントに固有の未着手の判断はもう無い。

**確度**: `src/slack/client.rs` は full-read。Web API メソッド 4 本 (`conversations.info`、`users.info`、`conversations.replies`、`conversations.history`) と `API_BASE`、`USER_TOKEN_PREFIX` による user token の門前払い (DR-0022 の実装にあたる)、`SlackFetchOutcome` が cap のヒットを呼び出し側へ運ぶ 3 bool、retry の増幅を避けるため `api_get_once` を直接呼ぶ 2 箇所 (`resolve_channel` と `fetch_user_name`) はこの読みによる。`src/slack.rs` は `classify()` のみ読み、残る 3 ファイルは unread。**この節の記述をメソッド名の粒度より細かくしないこと** — 上限値そのものと劣化通知への繋がりは `api-documentation.md` が持つ。

## brave

**責務**: Brave Search API クライアントと応答型。

**主なファイル**: `src/brave.rs`、`src/brave/client.rs` (369)、`src/brave/types.rs` (171)

**依存**: 横断リーフ (`classify`、`retry`、`body_limit`、`redacted`)。

**健全性**: healthy。バックエンドを Gemini Grounding から切り替えた判断が DR-0005、初期化を factory と `Result` ベースの https 検査へ統一した判断が DR-0007 にある。

**確度**: `src/brave/types.rs` は末尾側のみ読み (大半が `#[cfg(test)] mod tests`)、実装部は unread。`API_BASE` の 1 エンドポイントは grep-verified。

## search

**責務**: `research` サブコマンドの並列取得オーケストレーションとレポート整形。

**主なファイル**: `src/search.rs`、`src/search/engine.rs` (245)、`src/search/lang.rs`

**依存**: `brave` (検索)、`fetch` (取得)、横断リーフ (`envelope`)。scout の中で唯一、他のバックエンドを合成するコンポーネントである。

**健全性**: healthy。部分失敗を `DegradedReason` として envelope に載せる設計 (DR-0003) がここに集約されている。

**確度**: 全ファイル unread。`futures` の `stream::buffer_unordered` を使う並列取得と `degraded_reasons` への集約は `src/tools.rs` と `src/envelope.rs` の読みおよび依存関係から確認した範囲に留まる。

## 横断リーフ

**責務**: 全バックエンドが共有する 12 の単機能モジュール。エラー分類、終了コードと JSON envelope、リトライ、本文上限、Markdown/YAML 中和、秘密の型封じ込め、時計/乱数/トークン/文字コード/シグナルの注入点。

**主なファイル**:

| ファイル                                     | 持っているもの                                                                                                                                     |
| -------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/classify.rs` (99)                       | `Classification` と HTTP status 表、`from_reqwest` の timeout → transient → `Unknown` の順序 (DR-0003, DR-0011)                                    |
| `src/envelope.rs` (261)                      | `DegradedReason` 14 variant、`ErrorCode` 10 variant と `exit_code()`、`SuccessEnvelope` / `ErrorEnvelope` / `ErrorPayload` の serde 属性 (DR-0010) |
| `src/retry.rs`                               | `Retry-After` 遅延の集約点 (DR-0006)。`DEFAULT_MAX_RETRIES`                                                                                        |
| `src/body_limit.rs`                          | `read_body_capped` / `read_body_snippet`、および `MAX_API_RESPONSE_BYTES` (1 MiB)。`clippy.toml` がこれ以外の body 読み出しを禁じる                |
| `src/markdown.rs` (671)、`src/yaml.rs` (403) | エージェント向け出力の中和 (DR-0014)                                                                                                               |
| `src/redacted.rs`                            | `Redacted` 型による秘密の封じ込め (DR-0015)                                                                                                        |
| `src/clock.rs`、`src/rng.rs`                 | 時計と乱数の注入点。テストが実時間を待たないための seam (DR-0008)                                                                                  |
| `src/token_source.rs` (223)                  | トークン解決の優先順位と `spawn_gh` (DR-0018)                                                                                                      |
| `src/charset.rs`                             | 文字コード判定とデコード (DR-0013)                                                                                                                 |
| `src/signals.rs`                             | `InterruptSignal::exit_code` と graceful drain (DR-0017)                                                                                           |

**依存**: 他のリーフを最小限に参照するのみで、バックエンドへの import は持たない。この向きが循環の不在を支えている。

**健全性**: healthy。1 ファイル 1 関心が最も徹底されている層で、`src/classify.rs` は 99 行、`src/main.rs` に次いで小さい実装ファイルである。

**確度**: `src/classify.rs` と `src/envelope.rs` は full-read。`src/token_source.rs` は末尾側のみ読みで、`spawn_gh` の挙動は T-TOK004 の assertion から確認した。`src/body_limit.rs` は `MAX_API_RESPONSE_BYTES` の定義のみ確認。残る 8 ファイルは unread で、責務と DR の対応は `src/lib.rs` の `mod` 宣言と DR 索引による。
