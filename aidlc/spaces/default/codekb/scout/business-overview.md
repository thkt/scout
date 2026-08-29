# Business Overview — scout

このリポジトリは自分自身を既に文書化している。この CodeKB は一次ソースを写さず、どこに何があるかを指す索引として書く。数値と主張の出所は `reverse-engineering-timestamp.md` の `## 測定基準` にある commit と測定日に縛られる。

## この CLI が解決する問題

`scout` は、Web 上の情報をコーディングエージェントが読める形へ変換する単一の Rust CLI である。エージェントは HTML を読めるが、広告・ナビゲーション・スクリプトを含む生の HTML はトークンを浪費し、本文の所在も曖昧になる。scout はこの変換を 1 本のバイナリに閉じ、結果を Markdown か 1 行 JSON envelope で返す。

解く問題は 4 つに分かれる。

| 問題                                                          | scout の答え                                                                                       |
| ------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| 検索結果を得たいが、ブラウザも API キー管理も持ち込みたくない | Brave Search API を 1 エンドポイントで叩き、既定は URL を 1 行 1 件で返す (DR-0020)                |
| ページ本文だけを Markdown で欲しい                            | Readability 実装 (`dom_smoothie`) で本文を抽出し、`htmd` で Markdown へ変換する                    |
| GitHub リポジトリの構造とファイルを読みたい                   | GitHub REST v3 の GET 8 本だけを使い、ツリー / ファイル / README / issue / PR / release を整形する |
| Slack の permalink が指す会話を読みたい                       | Slack Web API の GET 4 メソッドでスレッドを復元し、mention を人名へ置換する                        |

## 一次コンシューマ

**想定利用者は人間ではなく AI コーディングエージェントである。** この前提が観測可能な形でコードに現れている箇所が 3 つある。

- `src/lib.rs` の `AGENT_HELP_HINT` — `--version` 実行時に tracing 経由で stderr へヒントを出し、エージェントを `--help` へ誘導する
- `src/lib.rs` の `after_help` — 終了コード表・環境変数・チューニング範囲を `--help` 本文に全部載せる。内容は `root_help_contains_exit_codes_and_environment` (T-H000) と `root_help_lists_scout_tuning_env_vars` (T-H010) が pin する
- `src/envelope.rs` の `DegradedReason` — 部分失敗を文字列パースなしに検出させるための機械可読な劣化理由 (DR-0003)

出力そのものがエージェントへの攻撃面になるという判断が DR-0014 (Output-Injection Defense for AI-Agent Consumers) に記録されており、`src/markdown.rs` と `src/yaml.rs` の中和処理がその実装にあたる。

## 提供する 6 つの能力

CLI サブコマンドは `src/tools/params.rs` の `enum Command` が定義する 6 本である。実際の分岐は `src/tools.rs` の `run()` にある。

| サブコマンド    | 能力                                                    |
| --------------- | ------------------------------------------------------- |
| `search`        | Brave Search API へ問い合わせ、URL 一覧を返す           |
| `fetch`         | 単一 URL を取得し、本文を Markdown 化して返す           |
| `research`      | 検索と複数ページ取得を束ね、1 本のレポートに畳む        |
| `repo-tree`     | GitHub リポジトリのファイルツリーをグロブで絞って返す   |
| `repo-read`     | GitHub 上の単一ファイルを復号して返す                   |
| `repo-overview` | リポジトリの README・issue・PR・release を 1 枚に束ねる |

Slack permalink の取得は独立したサブコマンドではない。`src/tools/query.rs` の `fetch` ハンドラが `parse_slack_url` を呼び、URL が Slack permalink の形なら `fetch_slack` へ振り分ける。契約面の詳細は `api-documentation.md` が持つ。

## ドメイン境界 — scout が引き受けないこと

境界は防御の設計と直結しているため、明示された「やらないこと」がそのまま安全性の根拠になっている。

- **内部ネットワークへは出ない。** ユーザー入力 URL は SSRF 防御の対象であり、名前解決の結果を接続時に再検査する (DR-0001, DR-0009, DR-0012)。proxy 経由の経路だけは名前解決由来の防御を proxy の egress control へ委譲する (DR-0023)
- **秘密を出力へ漏らさない。** トークンは `src/redacted.rs` の `Redacted` 型に封じ込める (DR-0015)。`gh auth token` サブプロセスの stderr はトークンを含みうるので破棄し、終了コードだけを報告する (DR-0018)
- **状態を持たない。** 永続ストア、キャッシュ層、設定ファイルのいずれも持たない。設定は環境変数と CLI 引数だけで、既定値と許容範囲は `--help` に載る (DR-0019)
- **書き込まない。** 外部 API への要求は全部 GET である。GitHub 8 本・Slack 4 メソッドのすべてが読み取りのみで、この一方向性が `api-documentation.md` の Consumed 表に列挙されている

## 一次ソースの所在

この CodeKB は下の 4 つを写さずに指す。値が食い違ったときに正しいのは常に右列である。

| 知りたいこと                           | 一次ソース                                                                                                                                        |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| なぜその設計にしたか、却下した案は何か | `docs/decisions/` の Decision Record 28 本 (MADR v4、全て `status: "accepted"`)。索引は `docs/decisions/README.md`                                |
| その決定を守っているテストはどれか     | 各 DR の Confirmation 節が `[T-XXX]` 形式のテスト ID を名指しする。ID 体系の規約は `src/test_support.rs` の crate doc                             |
| 実装ファイル単位の評価と未着手の判断   | `docs/audit/2026-08-11-rust-code-assessment.md` (v2.5.0 / commit `c0499fd` / 測定日 2026-08-17 基準)                                              |
| コーディング規約の本体                 | `.claude/rules/CONVENTIONS.md` が索引で、規約本体は `Cargo.toml` の lints、`clippy.toml` の reason、`src/test_support.rs` の crate doc に置かれる |

**ただし右列が常に新しいとは限らない。** `docs/audit/2026-08-11-rust-code-assessment.md` は v2.5.0 基準であり、この CodeKB が同じ対象を測り直して監査文書の記述を覆した箇所が 3 つある (E-1 / E-3 / E-4)。**その 3 項目についてはこの CodeKB の実測が新しく、`code-quality-assessment.md` の `## 技術的負債` が測定範囲つきで持つ。** 同じ理由で、`.claude/rules/CONVENTIONS.md` が `src/fetch/converter.rs` について書く「6 群」も上書き済みである。測っていない範囲では右列が正しいという原則はそのまま残る — DR の本文、テスト ID の規約、lint の deny リストはいずれも一次ソース側が持つ。

この索引方式は scout 自身の規約でもある。`.claude/rules/CONVENTIONS.md` は「一次ソースを写した時点で 2 箇所が食い違う」と定め、規約の所在だけを持って本体を書かない。
