# Outcome & Simplicity Audit: 2026-05-30

アウトカム整合・シンプルさ・open issue 方向性の横断監査。`.claude/OUTCOME.md` を基準に、コマンド表面の end-to-end 実証、複雑性コストセンターの計測、issue 群の Progressive Enhancement staging を行った。手法は OUTCOME 照合 + 実機実証 + issue 集約判定 + advisor adversarial challenge。

## Summary

| Metric | Value |
| ------ | ----- |
| 結論（向かう方向） | 正しい。capability 達成済み、backlog は残バグ + 内部品質 polish へ健全にシフト |
| Behavior 実証 | `scout fetch https://example.com` が中間要約なしで本文を Markdown 取得（実機 exit 0） |
| コマンド表面 | 6 コマンド全て「一次ソース取得」に収束。逸脱なし |
| アウトカムドリフト | 1 件（Slack が OUTCOME.md Behavior 未記載） |
| open issue 総数 | 19 |
| うち OUTCOME 直結 | ~6（SSRF #184/#193, OOM #186, timeout #185, escape #187, slack #188 + 新規 #198/#199） |
| うち直交する内部品質 | ~13（DI seam, alloc 削減, trait 昇格, 構造化ログ）。正当だが gold-plating 注意 |
| 複雑性コストセンター | CDP/js-rendering: 本体 562 行 + `cfg(js-rendering)` 42 箇所散在。default 無効の opt-in 隔離 |
| ディレクトリ最大深さ | 4（`src/fetch/cdp/launch/*_tests.rs`）。STRUCTURE.md 3 階層超だが LANG.md 分割 + framework override 範囲内 |
| テスト規模 | 438 関数（#[test]/#[tokio::test]） |

## 1. アウトカム整合性 — 高い（実証済み）

| 観点 | 判定 | 根拠 |
| ---- | ---- | ---- |
| Behavior 達成 | 達成 | `scout fetch https://example.com` が YAML frontmatter + clean Markdown を返し、中間 LLM 要約を挟まない。README "Read the sources, not a summary" と一致 |
| コマンド表面 | 整合 | search / fetch / research / repo-tree / repo-read / repo-overview が全て一次ソース取得に収束 |
| Non-goal 遵守 | 遵守 | MCP 提供なし、ローカルファイル処理なし、対話操作なし。js-rendering は SPA 本文取得であって対話操作ではないと README で切り分け済み |

### ドリフト: Slack（要修正）

- 実態: `scout fetch <slack-url>` として fetch に統合、README に `SLACK_TOKEN` 明記。API トークン認証であり Non-goal の「ログインフロー/対話操作」には該当しない。Slack スレッドは一次ソースであり、アウトカムの精神には合致する。
- 問題: OUTCOME.md の Behavior は「Web ページや GitHub リポジトリ」のみで Slack を名指ししていない。コードが文書を追い越している。
- 修正方向: Slack を削るのではなく、OUTCOME の Behavior に source type を明記する（web / GitHub / Slack）。

### フラグ: OUTCOME.md が `.gitignore` 配下

`.claude/` ごと gitignore されており、アウトカムへの整合が現状はチーム非可視の個人文書への整合に留まる。北極星が共有されていない点は、参照基準としての弱さ。共有の可否は運用方針のためユーザー判断。

## 2. シンプルさ判定

| 対象 | 判定 | 理由 |
| ---- | ---- | ---- |
| コア（search/fetch/research/repo-*） | シンプル | 最大ファイル 392 行（閾値 400 内）、素直な pipeline 構造、逸脱なし |
| CDP / js-rendering | keep（隔離は正しい） | 本体 562 行 + `cfg(js-rendering)` 42 箇所散在。chromiumoxide + nix + signal + process-group kill + orphan reap が複雑性の集中点。ただし default 無効の opt-in でコアのシンプルさは侵さない。SPA という最も薄いアウトカムスライスに複雑性が集中する構造は認識すべき。42 箇所散在は将来の simplify 候補 |
| ディレクトリ深さ | 許容 | 最大 4 階層は全て CDP launch のテストファイル。LANG.md の `sub.rs` + `sub/child.rs` 分割と framework override の範囲内。直近の split campaign は概ね可読性に寄与（最大 392 行に収束） |

## 3. issue 見直し — 規律ある backlog、優先順位の明示が必要

backlog 自体は健全。YAGNI gate 明記（#177「trigger 条件待ち」、#175「seam 1 個なので許容」）、トレーサビリティ完備（RC 番号、reviewer 収束数、critic-evidence verified）。close は推奨しない（trigger 条件の記録を捨てるため）。レバーは close ではなく sequencing と labeling。

Progressive Enhancement staging:

| Stage | issue | 扱い |
| ----- | ----- | ---- |
| Work / Resilient（バグ） | #184/#193 SSRF, #186 OOM, #185 timeout, #187 escape漏れ, #188 slack無音fallback, #198/#199 新規 | 本丸。次にやる。OUTCOME Constraint と品質に直結 |
| Fast（perf） | #190 decode_body 全コピー等（実害 verified） | 正当だが bug の後 |
| Flexible（trait/seam） | #174-177, #175, #191 | 2nd impl なし、YAGNI gate 付き。正しく deferred |

19 open のうち OUTCOME 直結は ~6 件、残り ~13 件は正当だが直交する内部品質。これは「機能未完成」ではなく「機能達成後の polish フェーズ」の姿勢。意図的なら正しいが、内部品質に偏った backlog は gold-plating に転びうるため、本丸バグの優先を明示しておく。

### 優先決着 1 件: #193

「TOCTOU DNS rebinding を IP pin で塞ぐか OUTCOME 制約に例外明記するか」。OUTCOME Constraint「全 fetch 経路で SSRF 防御を必須（private IP 帯への到達を遮断）」に直接関わる、文書判断を含むバグ。コード修正と OUTCOME 文書のどちらを動かすかの決定が保留されている。最優先で決める。

## 推奨アクション（最小）

| # | アクション | 性質 |
| - | ---------- | ---- |
| 1 | OUTCOME.md の Behavior に Slack を source type として明記 | 文書を実態に追従。OUTCOME 級のため文案確認 |
| 2 | #193 を最優先で決着（IP pin 実装 or OUTCOME 制約への例外明記） | OUTCOME に触る判断のため先送りしない |
| 3 | priority:low の trait/seam 系に deferred 運用（close せずラベル/整理で本丸バグと分離） | backlog の優先純度 |
| 4 | OUTCOME.md の `.gitignore` 除外を検討（北極星のチーム共有） | 運用方針、ユーザー判断 |

これ以上のコード変更・抽象化追加は不要。現状は「シンプルに保たれ、正しい方向を向いている」。
