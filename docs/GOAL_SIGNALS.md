# Goal signals: the trade-off policy, the Pending results lane, the policy sort

*Design for ADEV-269: "Spec: Goals, Guardrails and Signals". This note covers the rules that
live in the core. The spec of record, with the data model, the three HTTP contracts, the
server's behaviour, the surfaces and the gate, is astrid-web's
`docs/specs/GOALS_GUARDRAILS_SIGNALS.md`. The decision records are
[CONTRACTS.md D72–D75](./CONTRACTS.md). **Status: decided, not implemented.** Nothing in this
note is in the crates yet; each section names the task that builds it.*

## 1. Why

A project states goals and guardrails (`astrid_core::services::goals`, web's ADEV-179), and
the orchestration policy says what an agent may do alone (`astrid_rules::orchestration`,
[ORCHESTRATION_POLICY.md](./ORCHESTRATION_POLICY.md)). Jon's ask of 2026-11-25 connects them
to the outside: a goal's value is read from a metric source, an idea gets an estimate of its
effect on goals, guardrails and risk, a policy says what to do with that estimate, and built
work waits for an experiment or a verification to report.

Fetching is I/O and belongs to the server. Four things are rules, and every client must read
them the same way, so they belong here:

| Rule | Module | Contract | Decision |
|---|---|---|---|
| What label an estimate earns, and how two ideas order | `astrid_rules::tradeoff` (new) | `tradeoff.json` (new) | D72 |
| What an agent may do with a label | `astrid_rules::orchestration` | `orchestration.json` | D73 |
| When a task may wait for results, and where it goes after | `astrid_rules::results` (new) | `results.json` (new), `board.json`, `statuses.json` | D74 |
| The `policy` sort | `astrid_rules::filters` | `filters.json` | D75 |

All four are **core-first**: no client has them, so the core is canonical from the first
commit, as the Waiting lane (§8 of CONTRACTS.md) and the orchestration policy (D71) are. The
fixtures come from `cargo xtask gen-core-contracts`, and web copies them.

## 2. The trade-off policy (task 4a)

### Shape

A `tradeoff` key in `PolicyOverride`, beside `routing`, `autonomy` and `escalation`. It
resolves the same way: the default, then the project's, then the list's, field by field. An
absent key inherits.

```json
"tradeoff": {
  "goalOrder": ["g2", "g1"],
  "risk": { "reviewAt": "medium", "refuseAt": "high" },
  "guardrail": { "reviewAtBreachProbability": 0.1, "refuseAtBreachProbability": 0.5 },
  "experimentFirst": { "confidenceBelow": "medium", "riskAtLeast": "medium" },
  "minConfidenceToAct": "medium"
}
```

**The default is no policy.** With no `tradeoff` stored anywhere, `evaluate` answers "no
policy" and the caller draws nothing: no label, no sort option. Nothing changes for anyone
until a policy is stored, the same promise D71 makes.

These are thresholds, not a rule language. A stored value outside the vocabulary
(`"reviewAt": "scary"`, a probability outside 0–1) is a `badRequest`, never guessed.

### The input

`evaluate(policy, estimate, now)` takes the estimate as the server stores it:

- `goalImpacts`: per goal, `delta`, `low`, `high`.
- `guardrailImpacts`: per guardrail, `breachProbability`.
- `risk.level`: `low` | `medium` | `high`. A numeric `risk.score` may be present; the rule
  does not read it. Levels are the vocabulary the policy is written in.
- `confidence`: `low` | `medium` | `high`.
- `source`, `createdAt`, and the goals' `direction` (`up` | `down`).

`now` is an argument, as everywhere in `astrid-rules`. An estimate older than the caller's
staleness window is passed as stale; the rule does not own the window.

### The label

Four labels, evaluated strictest first. The first row that holds answers.

| Order | Condition | Label |
|---|---|---|
| 1 | Estimate missing, stale or unreadable | `needs-review` |
| 2 | A guardrail's breach probability or the risk level is at its refuse threshold | `dont` |
| 3 | A guardrail's breach probability or the risk level is at its review threshold | `needs-review` |
| 4 | Confidence is `low`, or the `experimentFirst` condition holds | `experiment-first` |
| 5 | Expected effect is in the better direction for at least one goal | `do-now` |
| 6 | Otherwise | `needs-review` |

Two floors apply whatever a policy stores, in the spirit of D71's destructive floor:

- **A section the estimator did not return is unknown, and unknown is never good.** No
  `guardrailImpacts` for a project that states guardrails is row 3, not a pass.
- **A built-in estimate is capped at `experiment-first`** (`source: "builtin_llm"`). It can
  be `dont` or `needs-review`; it is never `do-now`.

`evaluate` returns `{ label, reasons, sortKey }`. Each reason is a code plus the values that
produced it (`guardrailAtReview { guardrailId, breachProbability, threshold }`), so every
client words it in its own strings and none re-derives it.

### The order

`sortKey` is an ordered tuple, compared lexicographically:

1. label: `do-now`, `experiment-first`, `needs-review`, `dont`;
2. fewer guardrails at review;
3. for each goal in `goalOrder`, the impact band (the sign and size of `low`, in the goal's
   better direction), better first;
4. lower risk level;
5. lower cost;
6. newer.

**Nothing is weighted, multiplied or totalled.** There is no combined score to store or
draw. This is deliberate: web's `docs/DECISIONS.md` rule 7 forbids a score column on the
decision grid, and a sort that reduced an idea to one number would be that column by another
route. The tuple orders ideas by the owner's stated priorities and stops.

### What it does not decide

- **Whether to build the idea.** A label is a recommendation; a person may move any idea
  anywhere. Acting on a label without a person is §3.
- **The numbers.** The estimator's reply is data the server validated. The rule reads it.
- **The decision grid.** `astrid_rules::decisions` is untouched.

## 3. Acting on a label (task 4a, with the policy)

Three actions join `Action` (`Merge`, `Deploy`, `Migrate`, `Release`, `Spend`):

| Action | Autonomy key | Permits |
|---|---|---|
| `PromoteIdea` | `autonomy.promoteIdea` | moving a `do-now` idea from the inbox column to Ready |
| `ExperimentStart` | `autonomy.experimentStart` | calling an experiment service's `start` |
| `ExperimentStop` | `autonomy.experimentStop` | calling an experiment service's `stop` |

- Each defaults to `never`, as every action does (D71.2).
- `may` answers them as it answers the others: `allowed`, `ask`, `never`; an agent the list
  does not admit is `never`.
- One new floor: **`PromoteIdea` is `never` unless the estimate's label is `do-now` and its
  confidence is at least `minConfidenceToAct`.** The caller passes the label and confidence;
  a caller that passes neither is answered `never` (`noEstimate`). A built-in estimate
  cannot reach `do-now`, so it never promotes.
- `ask` is the server's approval card, bound to a digest of the estimate or experiment
  snapshot instead of a commit. That binding is web's.

A built-in estimate requested by an agent is answered by the existing `Spend`: it uses a
person's model key.

## 4. The Pending results lane (task 5a)

### A second preset

`PRESET_STATES` gains `("results", "Pending results", "Built and released; waiting for an
experiment or a verification to report.")` beside `testing`. Like `testing` it is opt-in per
board and its role is fixed, not minted.

**No board has it until a person adds it** (Jon, 2026-11-25: "we should not insert the
Pending results for all boards unless the user configures a verification or experimentation
column, but we should support it with easy configuration out of the box"). In core that means:

- `results` is in `PRESET_STATES` and nowhere else. It is not a default column, no board
  template carries it, and no rule adds a column to a board: not `results::settle`, not a
  write that links an experiment.
- `available_presets` is the whole of the "easy configuration": it offers `results` to any
  board that lacks it, and adding it is the same single write as adding `testing`. Whether to
  show the offer (web shows it only where goal signals are on, and once in context when a task
  is first linked to an experiment) is the client's, since core does not know the gate.
- The rules below apply to a task **in** a `results` column. On a board without one, a task
  with result conditions is an ordinary task: `results::settle` is not asked about it, and
  nothing completes it or hands it back.

**`testing` is unchanged.** R7 says a task in a preset "is not Waiting and needs no wait
condition", and boards rely on that. `results` is the first preset with rules, so R7's
sentence becomes true of `testing` only; CONTRACTS.md §8 is amended in the task that builds
this.

### The rules

Modelled on `waiting.rs`: a write resolver, a settle function, pure, with `now` passed in.
The caller passes the task's result conditions as it passes blockers to Waiting:
`[{ kind: "experiment" | "verification", state, verdict?, decideBy?, fetchedAt? }]`.

- **P1. Entry needs something to wait for:** at least one condition that is not concluded,
  or a verification whose `decideBy` is in the future. Otherwise the write is refused with
  `RESULTS_NEEDS_CONDITION`, which a client reads as "show the picker", the way it reads
  `WAITING_NEEDS_CONDITION`.
- **P2. Entry unassigns** and remembers the holder for the hand-back, as R2 and R8 do.
  Results are nobody's work while they are pending.
- **P3. Passed → Done.** Every condition concluded, with verdict `improving`, or a
  verification that ended with no guardrail breached and no goal regressed. `settle` answers
  "complete"; the caller completes through `TaskService::complete` (README rule 2).
- **P4. Not passed → hand back.** A verdict of `regressing`; `inconclusive` once `decideBy`
  has come; or a source that has not reported by `decideBy`. `settle` answers
  `waiting::hand_back` to the policy's escalation target: inbox column, open, assigned. The
  caller attaches the Decision (iterate, roll back, ship anyway).
- **P5. Not yet → stay.** A stale source before `decideBy` changes nothing.
- **P6. Leaving by hand is always allowed.** A person may drag the task anywhere; the
  conditions stay on the task.

Nothing in these rules rolls anything back.

**Mixed verdicts:** with several conditions, any `regressing` is P4; otherwise all must pass
for P3; otherwise P5 until the latest `decideBy`.

### Where the task sits meanwhile

R6 of the Waiting lane says finished work awaiting deploy stays in Doing. That still holds.
A task enters `results` when it is released and something is measuring it, which is after
R6's case.

## 5. The policy sort (task 4a)

`filters::sort_by_setting` gains a `"policy"` arm. It compares the `sortKey` the server
stored on each task's current estimate, through `display_order` for ties. A task with no
key sorts after every task with one, in `auto_order` among themselves.

- **The arm compares; it does not evaluate.** The key is computed once, on the server, when
  an estimate arrives or the policy changes. So the browser build needs `tradeoff` no more
  than it needs `orchestration` (D71.6), and its size budget
  ([BROWSER_DOOR.md](./BROWSER_DOOR.md)) is not spent.
- **Older clients are safe.** An unrecognised sort string already falls through to
  `auto_order` (`sort_by_setting`'s last arm: "auto, and anything a newer build
  introduced"), so a list set to `policy` reads as Auto on a client pinned before this.
- The wire type for a row gains an optional estimate summary (`label`, `sortKey`) in
  `model/`. Absent means unlabelled.

## 6. Where it runs

- `tradeoff` and `results` are features of `astrid-rules`, on by default, forwarded to
  astrid-web's **Node** package. That is where the server evaluates a policy and settles the
  lane.
- **Off in the browser build**, refused by name, as `orchestration` is.
  `scripts/check-wasm-packages.mjs` asserts both halves.
- The `policy` sort arm is in every build, the browser's included: it is a comparison.
- Native clients get all of it through `astrid-core`. They draw nothing new until task 7
  reads links, estimates and results into the cache (`services/goals.rs` and a new
  read-only service). Until then a native client shows a `results` column as any preset
  column and a `policy` sort as Auto.

## 7. What stays outside the core

- Calling a metric source, an experiment source or an estimator. HTTP is the server's.
- Statistics. Deltas, intervals and significance are the experiment service's.
- The built-in estimator's prompt and model call.
- Who may edit a policy or a connector (owners and admins, web's `lib/list-permissions.ts`),
  as D71 §8.5 leaves it.

## 8. Decisions taken, and the ones left open

Jon decided (2026-11-25): goals stay per project; the first release has open contracts only;
a policy ranks, labels and recommends, and acting on it is `allowed` / `ask` / `never`,
default `never`.

Decided here under the autonomy rule, each open to Jon's override:

1. **Decided (agent, 2026-11-25): four fixed labels and a lexicographic order, no score.**
   A weighted score is additive later; it would need rule 7 of web's DECISIONS.md revisited
   first.
2. **Decided (agent, 2026-11-25): a new `results` preset, not rules on `testing`.** Changing
   `testing` would refuse moves that boards make today. **Amended by Jon, 2026-11-25:** the
   preset is never added to a board automatically; a person adds it, in one step (§4).
3. **Decided (agent, 2026-11-25): passing results complete the task by rule.** The
   alternative, handing every result to a person, makes the lane a second inbox.
4. **Decided (agent, 2026-11-25): the rule reads `risk.level`, not `risk.score`.** Services
   disagree on what a score means; three levels are a shared vocabulary.
5. **Decided (agent, 2026-11-25): a built-in estimate is capped and never acts.**
6. **Open: per-goal thresholds** ("refuse anything that lowers g1 at all"). Today a goal
   can only be ranked in `goalOrder`. Additive when someone asks.
7. **Open: a project floor on `tradeoff`**, the same question D71 §8.2 left open for
   autonomy.

## 9. Sequencing

| Task | Does | After |
|---|---|---|
| 1b | `services/goals.rs` reads and keeps a goal's `metric` block and a guardrail's `metric` check, so a native save does not drop them | web's 1a |
| 4a | `tradeoff` module, the three actions, the `policy` sort arm, `tradeoff.json`, extended `orchestration.json` and `filters.json` | — |
| 5a | `results` preset and module, `results.json`, extended `board.json` and `statuses.json`, §8 amended | — |
| 7 | native read and cache of links, estimates and results | web's 6a, 6b |

4a and 5a are independent of each other and of web. Web re-pins after each
(`packages/astrid-rules/REVISION`), and `CONSUMERS.md` records the pins.
