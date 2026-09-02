# AI-DLC Workflows 2.0 導入記録

対象: scout main
実施日: 2026-08-28
更新日: 2026-09-02 (v2.7.0 へ載せ替え)
基準 commit: c43278f (v2.6.0)

scout の開発フローを AI-DLC Workflows 2.0 へ置き換えるために、engine を `.claude/` へ、workspace を `aidlc/` へ入れた。同梱の設定のうち 3 点を落として入れている。再インストールと撤去に必要な情報をここに残す。`/think` `/issue` `/build` `/audit` などの旧フローは `~/.claude` のグローバル資産なので、この導入では触っていない。

## 固定した upstream

| 項目 | 値 |
| ---- | -- |
| リポジトリ | https://github.com/awslabs/aidlc-workflows |
| タグ | `v2.7.0` |
| commit | `96b11d39028955d4f92375e783525db5275cdfd8` (2026-09-01) |
| AIDLC_VERSION | 2.7.0 |

`v2.7.0` は GitHub Release のタグで、同じ日に `main` が v2 の source of truth になった。再現するときは `git clone --branch v2.7.0` で取る。初回導入 (2026-08-28) は `v2` ブランチの commit `2fbee12` (AIDLC_VERSION 2.6.123) を使い、2026-09-02 に v2.7.0 へ載せ替えた。

## 2.7.0 への載せ替え (2026-09-02)

2.6.123 から 2.7.0 までで `dist/claude/` が変わったのは 4 ファイルで、いずれも `aidlc-state.md` が絶対パスを持つ問題 (upstream issue #937、2.6.124 で対処) と版番号である。2.7.0 自体は 2.6.x を新しい minor に束ねた版で、CHANGELOG は「2.6.124 から runtime の挙動を変えていない」と書く。

| ファイル | 変更 |
| -------- | ---- |
| `.claude/tools/aidlc-utility.ts` | 新規 state の `Project Root` を絶対パスではなく `.` で書く |
| `.claude/tools/aidlc-state.ts` | worktree state の `Worktree Path` を project-relative で書く |
| `.claude/knowledge/aidlc-shared/state-template.md` | `Project Root` の placeholder を上の形に合わせる |
| `.claude/tools/aidlc-version.ts` | `2.6.123` → `2.7.0` |

`aidlc/` の method tree、同梱 `.gitignore`、`.mcp.json`、`settings.json`、`aidlc-lib.ts` は上流側で 1 バイトも変わっていない。したがって上の 4 ファイルを `.claude/` へ上書きすれば `dist/claude/` を丸ごと差し替えたのと同じ状態になり、「同梱から外したもの」の 3 点、bun の絶対パス化、`aidlc-lib.ts` のローカルパッチはそのまま残る。上書き後に `/usr/bin/diff -rq` で v2.7.0 の `dist/claude/.claude` と突き合わせ、差分が scout 固有のファイルと `settings.json`・`aidlc-lib.ts` の 2 本だけであることを確かめた。

上書き後に `aidlc-utility.ts plugin-sync` を走らせた (2.7.0 は engine を差し替えるたびにこれを要求する)。plugin は入れていないので `no installed plugins; nothing to sync` で終わる。

既存の `aidlc-state.md` の `Project Root` は `/Users/thkt/GitHub/cli/scout` の絶対パスのまま残る。2.6.124 の CHANGELOG は「実行時に `projectRootFor` が `aidlc/` を探して再導出するので、この値は到達しない fallback であり移行は不要。engine が次にそのフィールドを書くときに置き換わる」としている。手では書き換えない。

2.7.0 の release note が Claude Code に求める `/hooks` での承認と完全再起動は、載せ替え後に人が行う。hook の command 文字列は変わっていない。

## 入れたもの

| 配置元 | 配置先 | 中身 |
| ------ | ------ | ---- |
| `dist/claude/.claude/.` | `.claude/` | skills 42、agents 14、hooks 18、tools、knowledge、aidlc-common、scopes 11、sensors 6、`CLAUDE.md`、`rules/aidlc.md`、`settings.json` |
| `dist/claude/aidlc/.` | `aidlc/` | `spaces/default/memory/` の method tree。`/aidlc --doctor` の workspace shell ready がこれを見る |

`.claude/` は scout の `.gitignore` が丸ごと無視している。scout の `.claude/rules/CONVENTIONS.md` `.claude/rules/CORRECTIONS.md` `.claude/OUTCOME.md` `.claude/agent-memory/` も元から untracked なので、engine を untracked のまま置くのはこのリポジトリの既存の扱いに揃う。AI-DLC が持ち込む `rules/aidlc.md` は既存 2 本とファイル名が衝突しない。

`aidlc/` は upstream 同梱 `.gitignore` の分割方針に従って git に載せる。per-user cursor (`aidlc/active-space`, `aidlc/spaces/*/intents/active-intent`) と machine-local runtime は無視し、method memory・intents.json・aidlc-state.md・audit shard・artifacts は commit する。同梱 `.gitignore` のうち Vite/Node 向けの boilerplate (`node_modules` `dist` `*.log` `.vscode/*` ほか) は取り込まず、`# AI-DLC` 以降だけを scout の `.gitignore` へ足した。`.claude/settings.local.json` の 1 行も、scout が `.claude/` を丸ごと無視するので落とした。

upstream issue #937 は `aidlc-state.md` が machine-local な絶対パスを共有 state へ書くと報告している。`aidlc-state.md` は workflow を 1 度走らせるまで生成されないので、初回の commit には載らない。それを含む最初の commit の前に中身を確認する。

## 同梱から外したもの

| 対象 | 理由 |
| ---- | ---- |
| `settings.json` の env 6 変数 (`CLAUDE_CODE_USE_BEDROCK`, `AWS_REGION`, `ANTHROPIC_DEFAULT_{FABLE,OPUS,SONNET,HAIKU}_MODEL`) | AWS Bedrock 固定を外し、現在の Claude Code のモデル設定のまま動かす。AWS 認証情報と Bedrock のモデルアクセス申請が不要になる。`AWS_AIDLC_DEFAULT_SCOPE=classic` は AI-DLC 側の設定なので残した |
| `settings.json` の `"model": "opus[1m]"` | 同上。session のモデルを AI-DLC が上書きしないようにする |
| `dist/claude/.mcp.json` | context7 (要 `CONTEXT7_API_KEY`) と AWS 系 4 本 (aws-mcp / aws-pricing / aws-iac / aws-serverless、要 uvx)。scout は Rust CLI で AWS へのデプロイ対象を持たない |

削除前の原本は `.claude/settings.json.aidlc-orig` に残してある。

## 同梱のまま有効にした挙動

`settings.json` が session 全体へ効かせるもの。AI-DLC を使わない作業中も効く。

| 項目 | 内容 |
| ---- | ---- |
| `permissions.allow` | `Read` `Edit` `Write` `Bash` `Glob` `Grep` `Task` `WebSearch` を素で並べる。`Bash` は無条件許可で、scout がこれまで `settings.local.json` に持っていた 4 本の限定 allow より広い |
| `effortLevel` | `xhigh` |
| `statusLine` | `aidlc-statusline.ts` へ差し替え |
| `companyAnnouncements` | session 開始ごとに 33 stage の表を出す |
| hook | PreToolUse ほか 18 本を登録する |

`.claude/CLAUDE.md` (18.7 KB) と `.claude/skills/aidlc/SKILL.md` (50.8 KB) が context に載る。

## workflow を張っていないときに guard が素通りするか

`aidlc-state-transition-guard` `aidlc-reviewer-scope` `aidlc-review-freeze` は PreToolUse の matcher が `Read|Edit|Write|Bash|Glob|Grep` で、`aidlc-plan-approval-guard` は `Edit|Write|Bash|Task` である。AI-DLC を起動していない普通の Rust 作業も全部この matcher に当たる。

評価用 worktree で、workflow 未起動の状態の PreToolUse payload を 6 本の hook へ流した。`Edit src/main.rs` `Write src/new.rs` `Bash cargo check` `Read src/main.rs` `Task` の 5 ケースすべてで exit 0・出力なし、つまり素通りだった。hook が実際に走ったことは doctor の `Hooks last fired: review-freeze ..., reviewer-scope ..., plan-approval-guard ...` の timestamp で確かめた。

## bun は mise shim を経由させず実体の絶対パスで呼ぶ

`bun` は `~/.local/share/mise/shims/bun` (mise 本体への hard link) に解決される。この shim 経由で `.claude/tools/aidlc-utility.ts` を実行すると、AI-DLC の出力ではなく package manager の選択 picker (`? Choose the agent >`) が stdout に出て exit 0 で終わることがある。成功した実行を 3 回挟んだ後にも再発したので、初回解決だけの現象ではない。実体 `~/.local/share/mise/installs/bun/latest/bin/bun` では再現しなかった。

空振りは exit 0 で標準エラーも空なので、hook が黙って何もしない状態と区別が付かない。`aidlc-plan-approval-guard` や `aidlc-state-transition-guard` が空振りすると、承認ゲートの無い AI-DLC が動いているように見える。

対策として `settings.json` の hook command 18 本と statusLine を実体 bun の絶対パスへ書き換えた。`permissions.allow` は絶対パス形と `bun` 形の両方を許可する。

hook が発火しているかは doctor の `Hooks last fired:` 行で見る。`Hook heartbeats: not yet fired` は「まだ発火していない」と「発火して空振りした」を区別しないので、この判定には使えない。

bun を上げると `latest` の実体が入れ替わる。その場合は「そのファイルが無い」と鳴って落ちるので、空振りより検出できる。

## Bash コマンドを改行で分けると runtime-graph の再構築 hook が空振りする

**`bun` という語とツールのパスは同じ行に置く。** 分けると hook が黙って起動せず、学びの儀式が動かなくなる。

`.claude/hooks/aidlc-rebuild-stage-graph.ts` は PostToolUse で Bash コマンドの文字列を読み、`aidlc-runtime.ts compile` を起動する。それが intent 記録直下の `runtime-graph.json` を書く。判定は `aidlc-lib.ts` の `classifyRuntimeCompileCommand` が持ち、`runtimeCompileReport` は `\bbun\b.*<harness>/tools/aidlc-orchestrate\.ts\b.*\breport\b` の 1 本である。`s` フラグが無いので `.*` は改行を越えない。

2026-08-29 の Reverse Engineering 実行で、承認コマンドを 1 行目 `BUN=$(mise which bun); cd <repo>`、2 行目 `"$BUN" .claude/tools/aidlc-orchestrate.ts report --result approved` の 2 行で出した。分類器は `pass` を返し、hook は起動せず、`runtime-graph.json` は作られなかった。`aidlc-learnings.ts surface` はそのファイルを要求するので失敗し、**学びの候補抽出と `persist` が走らないまま stage が承認された**。承認後は `surface` が `slug mismatch` を返すので、その stage の儀式はもう実行できない。

`"$BUN"` の使用は原因ではない。`mise which bun` の中に `bun` の語が入るため `\bbun\b` は当たる。効いているのは改行だけである。

| コマンドの形 | `classifyRuntimeCompileCommand` |
| ------------ | ------------------------------- |
| 1 行、`"$BUN"` を使用 | fire |
| 改行を挟む、`"$BUN"` を使用 | pass |
| 改行あり、`bun` とパスが同じ行 | fire |
| 1 行、ドキュメントどおりの形 | fire |

空振りは exit 0 で出力も無く、`.aidlc-hooks-health` の drop にも残らない。唯一の手がかりは `/aidlc --doctor` の `! [runtime-graph-missing]` 1 行で、これは advisory なので 51 passed / 0 failed のまま出る。

失われた儀式の代わりに、その stage で起きたことは `<record>/inception/reverse-engineering/memory.md` へ手で 5 件書き足した。観察ノートは stage 記録の一部として残るが、`project.md` の規則にはならない。

### 当てたローカルパッチ

`.claude/` は追跡対象外なので、このパッチは再インストールで消える。再現するときは下の diff を `.claude/tools/aidlc-lib.ts` へ当て直す。v2.7.0 の `aidlc-lib.ts` は 2.6.123 と同一なので hunk の行番号はそのまま当たり、上流の 3 本の正規表現も `\bbun\b` アンカー付きのままである。

`\bbun\b` のアンカーを外し、判定をコマンド全体からシェルのセグメント単位へ移した。アンカーは「ツールのパスの隣に `bun` という語が同じ行にある」ことを要求しており、絶対パス指定・`"$BUN"` 変数・複数行コマンドのいずれでも外れる。ツールのパスと動詞だけで呼び出しは identify でき、セグメント単位にすることで隣のコマンドが判定を広げることを防ぐ。

```diff
@@ -1502,27 +1502,38 @@
 const runtimeCompileHarnessPattern = KNOWN_HARNESS_DIRS
   .map((dir) => dir.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))
   .join("|");
+// LOCAL PATCH (scout): the `\bbun\b` anchor and whole-command matching made
+// this gate miss any invocation where the runtime is not the literal word
+// `bun` next to the tool path on one line - an absolute path, a `"$BUN"`
+// variable, or the call on a later line of a multi-line command. The miss is
+// silent (exit 0, no output, no hooks-health drop), and it stops
+// runtime-graph.json from ever being compiled, which in turn makes
+// `aidlc-learnings.ts surface` fail and the learnings ritual never run. The
+// tool path plus its verb identifies the call on its own; matching per shell
+// segment keeps an unrelated neighbouring command from widening the match.
 const runtimeCompileTool = new RegExp(
-  `\\bbun\\b.*(?:${runtimeCompileHarnessPattern})/tools/aidlc-(state|jump|bolt|unit|utility)\\.ts\\b`,
+  `(?:${runtimeCompileHarnessPattern})/tools/aidlc-(state|jump|bolt|unit|utility)\\.ts\\b`,
 );
 const runtimeCompileReport = new RegExp(
-  `\\bbun\\b.*(?:${runtimeCompileHarnessPattern})/tools/aidlc-orchestrate\\.ts\\b.*\\breport\\b`,
+  `(?:${runtimeCompileHarnessPattern})/tools/aidlc-orchestrate\\.ts\\b.*\\breport\\b`,
 );
 const runtimeCompileSelf = new RegExp(
-  `\\bbun\\b.*(?:${runtimeCompileHarnessPattern})/tools/aidlc-runtime\\.ts\\b`,
+  `(?:${runtimeCompileHarnessPattern})/tools/aidlc-runtime\\.ts\\b`,
 );
 
 export function classifyRuntimeCompileCommand(
   command: string,
 ): "reject" | "fire" | "pass" {
-  const invokesRuntime = shellCommandSegments(command)
+  const segments = shellCommandSegments(command);
+  const invokesRuntime = segments
     .some((segment) => /^\s*aidlc\s+runtime\b/.test(segment));
   if (runtimeCompileSelf.test(command) || invokesRuntime) {
     return "reject";
   }
   if (
-    runtimeCompileTool.test(command) ||
-    runtimeCompileReport.test(command) ||
+    segments.some((segment) =>
+      runtimeCompileTool.test(segment) || runtimeCompileReport.test(segment)
+    ) ||
     /\baidlc\s+(?:state|jump|bolt|unit)\b|\baidlc\s+(?:status|doctor|version|help)\b|\baidlc\s+scope\s+change\b|\baidlc\s+config\s+set\b/.test(command) ||
     /\baidlc\s+report\b|\baidlc\s+orchestrate\s+report\b|\baidlc\s+next\b.*\breport\b/.test(command)
   ) {
```

**引き換えに偽陽性を受け入れている。** `echo "... .claude/tools/aidlc-orchestrate.ts report ..."` のような言及もセグメントに含まれれば fire する。代償は不要な `aidlc-runtime.ts compile` が 1 回走ることで、compile は audit shard と memory ファイルから決定的に組み立てるので結果は変わらない。パッチ前の偽陰性 (グラフが一度も作られず儀式が黙って死ぬ) より軽い。

分類器の判定は 8 ケースで確認した。失敗していた形 (改行 + `"$BUN"`)、ドキュメントどおりの 1 行の形、絶対パス指定の 3 つが fire、再帰ガードの 2 つが reject、無関係な 2 つが pass、上記の偽陽性 1 つが fire。

## 検証

```
bun .claude/tools/aidlc-utility.ts doctor
```

2026-08-28 の結果は 50 passed / 0 failed。2026-09-02 (v2.7.0 へ載せ替え後) は 51 passed / 0 failed。advisory は `hook-heartbeat-frozen` 4 件で、直近 58〜64 時間 AI-DLC の stage を動かしていないことによる。

## 使い方

session 内で `/aidlc --doctor` -> `/aidlc --status` -> `/aidlc <やりたいこと>`。

初回は `/aidlc --scope express <やりたいこと>` を勧める。express は RE・RA・CG・BT の 4 stage で engine を端から端まで通す。`AWS_AIDLC_DEFAULT_SCOPE=classic` のままだと INCEPTION 9 + CONSTRUCTION 7 + OPERATION 7 の 23 stage が `effortLevel: xhigh` で走り、reverse engineering が scout の 29,000 行超の Rust に当たる。classic は出力の質を評価する段になってから使う。

## 撤去

`.claude/` 側は AI-DLC が持ち込んだ 12 個の名前だけを消す。scout 既存の `OUTCOME.md` `rules/CONVENTIONS.md` `rules/CORRECTIONS.md` `agent-memory/` `settings.local.json` `tools.json` `workspace/` `worktrees/` `dead_code_analysis.md` は残す。

```
cd /Users/thkt/GitHub/cli/scout/.claude
mv agents aidlc-common hooks knowledge scopes sensors skills tools \
   CLAUDE.md settings.json settings.json.aidlc-orig settings.local.json.example rules/aidlc.md ~/.Trash/
cd .. && mv aidlc ~/.Trash/
```

`.gitignore` の `# AI-DLC` 節も外す。
