<!-- INVARIANT: examples are single-line HTML comments so a fresh template parses to total=0 (MEMORY_EMPTY). Do NOT un-comment or split across lines. t100 guards this. -->
> This file is kept up to date automatically while the stage runs. Add observations at the review step, not by editing here directly.

## Interpretations
<!-- example: 2026-05-29T10:14:32Z — chose REST over GraphQL; the consuming team only needs CRUD, revisit if subscriptions land -->

## Deviations
<!-- example: 2026-05-29T10:14:32Z — skipped the optional caching layer the stage prose suggested; the dataset is small enough that it adds risk -->

## Tradeoffs
<!-- example: 2026-05-29T10:14:32Z — picked TDD over BDD this run; the team is unit-first and the domain is well-understood -->

## Open questions
<!-- example: 2026-05-29T10:14:32Z — confirm the retention window with compliance before the next stage hardens the schema -->

- 2026-08-29T05:36:00Z — 単体実行が intent 記録を要求して完走できなかったので、記録を作ってから走らせ直した; `/aidlc-reverse-engineering` は「main workflow を進めない」と説明されるが、link receipt が active intent を要求する。エラーは開発者のスキャン完了後に出るため、スキャン 1 回分が捨てになった。
- 2026-08-29T05:36:00Z — 再利用を選ぶと stage が skipped になり承認ゲートが開かないと分かった時点で focused scan へ切り替えた; 人間の目的は承認ゲートを見ることだったので、選択肢の帰結を伝えて選び直してもらった。stage 定義の「If every repo is reused on an ordinary workflow run, report the stage as skipped」がその分岐。
- 2026-08-29T05:36:00Z — focused の対象を監査項目を抱えた未読 3 ファイルと `renovate.json` に絞った; 全体再スキャンは既に 20 件検証済みの内容を作り直すだけで、カバレッジの穴は埋まらない。E-1 / E-3 / E-4 と renovate の 3 規則がいずれも「監査文書の引き写し」か「未確認」のまま残っていた。
- 2026-08-29T05:36:00Z — 行番号付きコード参照 8 箇所をシンボル名へ直させた; DR-0028 が「行番号はコードが動くと別の宣言を指し、ずれたことが読者に見えない」としてシンボル名参照を規約にしている。DR-0028 の適用対象は DR だが同じ壊れ方をする。
- 2026-08-29T05:36:00Z — 学びの候補抽出 (`aidlc-learnings.ts surface`) が `runtime-graph.json` 不在で動かなかった; doctor も `[runtime-graph-missing]` を advisory として挙げる。承認ゲート必須の質問は手で出したが、候補の自動抽出と `persist` は走っていない。
