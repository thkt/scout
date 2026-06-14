# Slack #188 実装後監査: 2026-06-14

#188 の Slack 修正（`src/slack.rs`, `src/slack/client.rs` の `main..HEAD` diff、5 commit）を対象とした実装後監査。対象変更は ①無音 ID フォールバックへの `warn!` 追加、②`conversations.replies` のページネーション、③`users.info` lookup の上限 cap + 著者優先、④reply parent の重複排除。手法は reviewer fan-out → critic-audit (challenge) → critic-evidence (verify) → team-integration。pre-flight は `cargo test`（54 件 green）、`cargo fmt --check`、`cargo clippy`（いずれも clean）。

## Summary

| Metric                             | Value                                                                                                                    |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| 結論                               | #188 のスコープ内に修正すべき欠陥なし。変更は issue 推奨に整合し、テストも green                                         |
| pre-flight                         | test 54 件 pass / fmt clean / clippy clean                                                                               |
| ① warn 採用の妥当性                | 妥当。Result 化は呼び出し側 4 箇所の波及を生むのに対し、`String` 戻り値 + `warn!` は等コストで観測性のみ追加。Occam 適合 |
| ② page 上限 50・dedup・cursor 終端 | 正しい。`has_more` && cursor 非空で継続、`ts` を `HashSet` で dedup、上限到達時は `warn!` + `Ok(truncated)`              |
| ③ cap=50・著者優先                 | ロジックは正しい。著者を先に take し残枠を mention で埋める。Tier-4 (50 req/min) 整合                                    |
| 確定した actionable                | 0（スコープ内）。スコープ外の既存課題 1 件（F1）を申し送り                                                               |
| F1（既存・スコープ外）             | thread 切り詰め時の `not found in thread` が exit 64 (UsageError) に誤分類。該当行は 521b488b 由来で #188 diff 外        |

## ① 無音 ID フォールバックへの warn 追加 — 妥当

issue は「無音フォールバックを Result 化」を推奨していたが、採用された `String` 戻り値 + `warn!`（`resolve_channel` / `fetch_user_name`, client.rs 226-258）は以下の理由で妥当。

| 観点           | 判定                                                                                                            |
| -------------- | --------------------------------------------------------------------------------------------------------------- |
| 観測性         | `warn!(channel/user, "...falling back to raw ID")` で無音性は解消。運用ログに残る                               |
| コスト         | Result 化は呼び出し側の `?` 伝播・エラー型分岐を 4 箇所に強制。`String` 維持は等機能で indirection を増やさない |
| アウトカム整合 | name 解決失敗でも raw ID で本文取得を継続でき、「一次ソース取得」の Behavior を壊さない                         |

Result 化が優位になるのは「name 解決失敗を fetch 全体の失敗として扱う」仕様変更時のみ。現仕様（best-effort 解決）では `warn!` が最小。

## ② conversations.replies ページネーション — 正しい

`fetch_replies`（client.rs 285-326）。

| 要素            | 検証結果                                                                                                                                              |
| --------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| cursor 終端判定 | `MessagesBody::next_cursor`（slack.rs 152-166）が `!has_more` または cursor 空/欠落で `None` を返す。無限ループなし                                   |
| dedup           | thread parent が各 reply ページ先頭に再掲される Slack 仕様に対し、`ts` を `HashSet` で `seen.insert` 判定し重複排除（commit 140e16d, T-SK045 で固定） |
| ページ上限      | `SLACK_MAX_REPLY_PAGES=50`。上限到達時は `warn!` + `Ok(messages)` で truncated を返す（client.rs 319-325）                                            |

## ③ users.info lookup cap + 著者優先 — ロジック正しい

`fetch_message`（client.rs 367-422）。

| 要素     | 検証結果                                                                                                                                                   |
| -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| cap 値   | `SLACK_MAX_USER_LOOKUPS=50`。Slack `users.info` は Tier-4 (50 req/min)。1 メッセージ取得あたり 50 lookups は分あたり上限に整合                             |
| 著者優先 | cap 超過時 `authors.into_iter().take(50)` で著者を先取りし、残枠を mention で充填（commit ebaf4e4, T-SK044）。著者名は本文表示に必須なので優先順位は正しい |
| 並列度   | `SLACK_USERS_CONCURRENCY=5` で `users.info` を並列。バースト緩和                                                                                           |

注記（F4・スコープ外）: `authors` は `HashSet` のため著者数 > 50 のとき `take(50)` の取得順が非決定的。ただしこれは「どの 50 著者を引くか」の順序非決定であって著者優先ロジック自体の誤りではない。実害は稀（単一メッセージに 50 超の distinct 著者）で、影響はテスト再現性に限られる。

## F1: thread 切り詰め時のエラー誤分類（既存・#188 スコープ外）

`fetch_replies` がページ上限 50 で truncated を `Ok` で返す（②）と、対象 `ts` が切り詰められたページにある場合 `extract_target` が `None` を返し、`format!("message {} not found in thread", ts)`（client.rs 415-418）が生成される。この文字列は `classify()`（slack.rs 59-60）の NOT*FOUND アーム（`"message not found"` ＝接尾辞 `in thread` なし、の完全一致）にマッチせず `*` に落ち、**UsageError (exit 64)** に分類される。実態は「対象が見つからない/切り詰め」なので **NotFound (exit 66)** が適切。

- 既存性: 該当行は `git blame` で commit 521b488b（2026-05-30）。#188 の 5 commit（aba8998..140e16d）の外。
- #188 の関与: pagination は到達範囲を 1 ページ → 50 ページに拡張しただけで、誤分類自体は導入していない（pagination 前も page 1 外の target は同じ文字列で失敗していた）。
- 判定: #188 のスコープ内修正ではない。修正するなら別タスク（文字列を `"message not found"` に寄せる、または classify に接頭辞マッチを追加）。

## 結論

#188 の変更 ①〜④ はいずれも issue 推奨に整合し、ロジックは正しく、テストは green。スコープ内に修正すべき欠陥は検出されなかった。F1 は #188 が導入した欠陥ではなく既存の誤分類で、扱いはユーザー判断に委ねる。
