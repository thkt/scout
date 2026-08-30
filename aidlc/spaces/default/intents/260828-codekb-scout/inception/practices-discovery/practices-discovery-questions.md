# Practices Discovery — Questions

対象は `aidlc/spaces/default/memory/team.md` の 5 セクション。Way of Working、Deployment、Code Style の 3 つはリードの下書きと 3 者のレビューが証跡から確定させたので、質問は残る 3 点に絞る。

## Q1: Walking Skeleton

薄い end-to-end のスライスを最初に作りますか。walking skeleton とは、全体を端から端まで一度通す最小版を先に作り、実際の機能を入れる前に部品同士が繋がることを確かめるやり方です。

現在のスコープ `classic` は `skeleton: on` を宣言していますが、scout は既に 6 サブコマンドが動いている CLI で、繋ぐべき部品は繋がっています。

- A. 使わない。既存の CLI なので通すべき経路は既に通っている
- B. 使う。スコープの宣言どおり、最初の作業は薄い縦切りにする
- X. Other (please specify)

[Answer]: A

## Q2: Testing Posture — テストと実装の順序

テストは実装の前に書きますか、後に書きますか。

計測結果を先に出します。squash-merge 済みの PR も `gh api repos/thkt/scout/pulls/<n>/commits` でコミット一覧が取れるため、PR 内の順序は失われていません。2 つの窓を別々の方法で測った結果、**判別できた 24 PR のうち 20 PR で実装コミットがテストコミットに先行**していました。Red を単独のコミットとして先行させた PR は 0 件です。ただし実装とテストを同一コミットに畳んだ PR が多数あり、Green で畳んだ TDD とは区別が付きません。

この値が `team.md` の `**Methodology**` と `**Ordering**` になり、今後の Code Generation ステージがそれを読んで実装の順序を決めます。

- A. test-after。実装してからテストを書く。計測結果のとおり
- B. tdd。テストを先に書いて赤にしてから実装する
- C. custom。層によって変える (例: 統合テストは先、ユニットテストは後)
- X. Other (please specify)

[Answer]: B

## Q3: リリースのタグを誰が打つか

`v*` タグを push すると 4 ターゲットのクロスビルドが走り、GitHub Release が作られ、Homebrew tap (`thkt/homebrew-tap`) が自動更新されます。バージョンアップの commit の後にタグが続くことは確認できましたが、**タグ push 自体を何が行っているかがリポジトリ内に見つかりません** (release-please のような自動化は無し)。

- A. 人が手で `git tag` して push する
- B. リポジトリ外の仕組み (ローカルスクリプト、別リポジトリの CI など) が打つ
- X. Other (please specify)

[Answer]: A

## Consolidated Summary Confirmation

- Looks correct
- Request changes

[Answer]: Looks correct
