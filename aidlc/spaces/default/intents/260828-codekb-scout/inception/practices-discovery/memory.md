<!-- INVARIANT: examples are single-line HTML comments so a fresh template parses to total=0 (MEMORY_EMPTY). Do NOT un-comment or split across lines. t100 guards this. -->
> This file is kept up to date automatically while the stage runs. Add observations at the review step, not by editing here directly.

## Interpretations
<!-- example: 2026-05-29T10:14:32Z — chose REST over GraphQL; the consuming team only needs CRUD, revisit if subscriptions land -->

- 2026-08-29T09:35:00Z — 3 者のレビューを互いに見えない形で並行させたところ、下書きの結論 1 件が覆った; 下書きは「squash-merge が PR 内順序を畳むのでテストと実装の前後は証跡から判定できない」としていたが、品質レビューが `gh api repos/<owner>/<repo>/pulls/<n>/commits` で squash 済み PR のコミット一覧が取れることを示し、24 PR を実測した。人への問いが「証拠が無いので聞く」から「実測はこう出た、意図を確認したい」へ変わった。

## Deviations
<!-- example: 2026-05-29T10:14:32Z — skipped the optional caching layer the stage prose suggested; the dataset is small enough that it adds risk -->

- 2026-08-29T09:35:00Z — `practices-discovery-timestamp.md` を stage 文の「one line」ではなく 2 見出しで書いた; `required-sections` sensor が全 markdown 出力に `##` 見出し 2 つ以上を要求するため、1 行では sensor が落とす。必須の `Discovered: <ISO-8601> at commit <hash>` 行はそのまま含めている。stage 文と sensor の要求が食い違う箇所。
- 2026-08-29T09:35:00Z — 委譲したリードが `practices-event` の発行をガードに止められた; `aidlc-state-transition-guard.ts` が委譲エージェントの状態変更を拒否する設計どおりの挙動で、指揮側が代わりに発行した。stage 文の Step 5 は発行をリードの作業として書いているが、実際には指揮側の担当になる。

## Tradeoffs
<!-- example: 2026-05-29T10:14:32Z — picked TDD over BDD this run; the team is unit-first and the domain is well-understood -->

- 2026-08-29T09:35:00Z — GitHub Actions の SHA pin ルールを `## Mandated` へ入れず記述へ格下げした; zizmor が finding を出したときに job を落とすのか code-scanning alert に留まるのかを確認できていない。強制点が不明な規則を ALWAYS として書くと、強制されていないものを強制されていると読ませる。
- 2026-08-29T09:35:00Z — テストの方針が実測と逆方向になった; 実測は 24 PR 中 20 PR で実装先行、Red 単独先行は 0 件だったが、人は `tdd` を選んだ。`team.md` には意図として `tdd` を書き、実測との差を `evidence.md` に残す扱いにした。測定を意図で上書きするのではなく、両方を別の場所に置く。

## Open questions
<!-- example: 2026-05-29T10:14:32Z — confirm the retention window with compliance before the next stage hardens the schema -->
