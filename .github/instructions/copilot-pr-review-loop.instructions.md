---
applyTo: '**'
---

# Copilot PR Review Loop

A verified workflow for driving a pull request through repeated rounds of
GitHub Copilot code review until convergence ("no new comments"), addressing
findings that are real and low-risk while declining over-engineering or
purely hypothetical issues.

This was developed end-to-end on `microsoft/intelligent-terminal#122`
across 8 review rounds.

## When to use this workflow

- The PR is functionally complete and you want a final correctness pass.
- You want a paper trail of which Copilot findings were applied vs. declined,
  with rationale.
- You have time for ~5-7 minutes between rounds (typical Copilot latency).

Don't use it for trivial PRs (typos, comment-only changes), or while the PR
is still actively being designed — wait until the structure is stable.

## Loop overview

```
┌─────────────────────────────────────────────────────────────┐
│  1. Request Copilot review                                  │
│  2. Wait ~5-7 min, poll for new review                      │
│  3. Read open, non-outdated Copilot threads                 │
│  4. Triage each finding (fix vs. decline)                   │
│  5. Implement accepted fixes (build + run tests)            │
│  6. Commit + push                                           │
│  7. Reply + resolve each thread (with rationale either way) │
│  8. Goto 1                                                  │
│                                                             │
│  Terminate when: a round returns "no new comments"          │
│  Final step:    batch-resolve any remaining `isOutdated`    │
│                 Copilot threads                             │
└─────────────────────────────────────────────────────────────┘
```

## Step-by-step

### 1. Request a Copilot review

Use the `gh` CLI — the GraphQL `requestReviews` mutation no longer accepts
`botLogins` and REST `requested_reviewers` rejects bots with 422:

```powershell
gh pr edit <pr-number> --add-reviewer copilot-pull-request-reviewer
```

This is idempotent — re-running it triggers a fresh review.

### 2. Wait, then poll for the latest review and open threads

Copilot typically posts a new review within 3-6 minutes; allow up to 10.
Don't poll faster than every ~3 minutes — there's no progress signal and
spamming wastes API budget.

```powershell
Start-Sleep -Seconds 360
gh api graphql -f query='query{
  repository(owner:"<owner>",name:"<repo>"){
    pullRequest(number:<pr>){
      reviews(last:3){ nodes{ author{login} submittedAt state } }
      reviewThreads(last:30){
        nodes{
          id isResolved isOutdated
          comments(first:1){ nodes{ author{login} body path line createdAt } }
        }
      }
    }
  }
}' > round.json
```

Then filter for **open AND non-outdated** Copilot threads (Node one-liner):

```powershell
node -e "const d=JSON.parse(require('fs').readFileSync('round.json','utf8'));
for(const t of d.data.repository.pullRequest.reviewThreads.nodes){
  if(!t.isResolved && !t.isOutdated){
    const c=t.comments.nodes[0];
    console.log(t.id, c.author.login, c.path+':'+c.line);
    console.log('  '+c.body.slice(0,400).replace(/\n/g,' '));
  }
}"
```

Outdated threads (Copilot's earlier comments that point at lines you've
since rewritten) are irrelevant — only consider `!isOutdated`.

### 3. Triage each finding

Apply these criteria. **Be honest** — the goal is correctness, not appeasement.

**Fix when:**
- Real correctness bug (use-after-free, race that drops user intent,
  gating logic that skips legitimate transitions, link dependency the
  project doesn't declare).
- Cross-cutting concern with a clean, local fix (e.g. moving a mutex
  one level up).
- Documentation / test plan out of sync with implemented behavior.

**Decline when:**
- Purely hypothetical race needing cross-class plumbing (e.g.
  "what if FRE shared TerminalPage's mutex"), where the actual exposure
  is negligible and the fix would significantly complicate the design.
- Style / naming / formatting (Copilot is told not to raise these, but
  sometimes does).
- Suggestion to add abstractions ("introduce a strategy pattern") that
  don't pay for themselves at current scale.

Always **state your reasoning** in the reply, whether you fix or decline.
This makes the PR self-documenting for future maintainers and for the
next Copilot review (which will see your replies).

### 4. Implement fixes — one focused commit per round

Keep commits granular: one commit per review round (or per finding if
the round had multiple unrelated findings). This makes the PR history
narrate the review evolution.

For projects with **uncommitted local build patches** (e.g. toolchain
overrides), stash before committing and restore after:

```powershell
git stash push -m "local-build" -- <paths-to-stash>
git add <files-you-changed>
git commit -m "Short title" -m "Body explaining the finding and fix" `
           -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
git push
git stash pop
```

**Always include the Copilot `Co-authored-by` trailer** when the fix
came from a Copilot finding.

`git stash push` syntax pitfall: `-m` must come **before** `--`. The form
`git stash push -- <paths> -m <msg>` does NOT work.

### 5. Build and test before pushing

Don't push a fix you haven't compiled. If the project has unit tests for
the changed code, re-run them. A "fix" that breaks the build wastes
another full review cycle.

### 6. Reply to and resolve each thread

Reply first (explain what you did + cite the commit), then resolve.

```powershell
$body = "Did X because Y. Fixed in <commit-sha>."
gh api graphql -f query='mutation($tid:ID!,$body:String!){
  addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$tid, body:$body}){
    comment{ id }
  }
}' -F tid=<thread-id> -F body=$body

gh api graphql -f query='mutation($tid:ID!){
  resolveReviewThread(input:{threadId:$tid}){ thread{ isResolved } }
}' -F tid=<thread-id>
```

For **declined** findings, the reply explains why you're not fixing it —
then still resolve the thread. Leaving threads open without explanation
clutters the PR and signals you're avoiding the feedback.

### 7. Request the next round and loop

Go back to step 1. Each round, Copilot sees:
- the new diff,
- your replies on prior threads,
- the updated PR description.

So your replies actively shape what the next round will surface.

### 8. Convergence

When a round comes back with `"Copilot reviewed N out of N changed files
in this pull request and generated no new comments."` and the open-threads
list is empty, you're done.

### 9. Cleanup: batch-resolve outdated Copilot threads

Even after convergence, the PR may show old `isOutdated: true` Copilot
threads still listed as open. They're already addressed by later commits,
but they clutter the conversation tab. Batch-resolve them:

```powershell
gh api graphql -f query='query{
  repository(owner:"<owner>",name:"<repo>"){
    pullRequest(number:<pr>){
      reviewThreads(last:50){
        nodes{ id isResolved isOutdated comments(first:1){ nodes{ author{login} } } }
      }
    }
  }
}' > all.json

node -e "const d=JSON.parse(require('fs').readFileSync('all.json','utf8'));
const ts=d.data.repository.pullRequest.reviewThreads.nodes
  .filter(t=>t.isOutdated && !t.isResolved
             && t.comments.nodes[0].author.login==='copilot-pull-request-reviewer');
for(const t of ts) console.log(t.id);" | ForEach-Object {
  gh api graphql -f query='mutation($tid:ID!){
    resolveReviewThread(input:{threadId:$tid}){ thread{ isResolved } }
  }' -F tid=$_
}
```

## Things that do NOT work (verified)

- **GraphQL `requestReviews` with `botLogins`** — the API rejects it with
  `"InputObject 'RequestReviewsInput' doesn't accept argument 'botLogins'"`.
  Use `gh pr edit --add-reviewer copilot-pull-request-reviewer` instead.
- **REST `POST /repos/.../pulls/<n>/requested_reviewers`** with the Copilot
  bot login — returns 422 because bots aren't repo collaborators.
- **Polling more often than every ~3 min** — no progress signal exists,
  and Copilot reviews are not faster for being asked more often.

## Anti-patterns to avoid

- **Auto-accept every finding.** Copilot will sometimes suggest changes
  that materially complicate the design for a hypothetical edge case.
  Push back with a written rationale.
- **Bundle multiple rounds into one commit.** You lose the audit trail
  of which finding drove which change, and any later bisect becomes much
  harder to reason about.
- **Resolve a thread without replying.** The next reviewer (human or bot)
  has no record of why the issue was considered addressed.
- **Skip the build step.** "Looks right" is not the same as "compiles
  and passes tests." A broken push wastes a full review cycle.
- **Treat spell-check / format-check findings the same as code-review
  findings.** Those are separate CI signals and follow project-specific
  policies (e.g. this repo: reword for English words, don't add to
  `expect.txt`).
