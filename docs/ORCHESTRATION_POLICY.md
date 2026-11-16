# The orchestration policy

*Design for AWTD-1685: "Orchestration policy as rules: one policy per project, overridable per
list". The rules are `astrid_rules::orchestration`. The contract is
`contracts/fixtures/orchestration.json`, which is core-first. The decision record is
[CONTRACTS.md D71](./CONTRACTS.md). Storage, API, settings UI and the generated skill are
astrid-web's half, "Orchestration policy: stored on the project, editable in settings, readable by
agents" (AWTD-1693).*

## 1. Why

Three questions decide what an agent does on a board. Until now prompt text, env flags and repo
scripts answered them:

- **Who takes the work.** `get_agent_queue { includeUnassigned: true }`: any loop may claim an
  unassigned Ready task, and the first claim wins.
- **What an agent may do alone.** astrid-web's generated skill hard-codes "Never merge or deploy"
  (`lib/agent-skill/loop-recipes.ts`). Astrid's own loops follow `docs/FIXALL_WORKFLOW.md` →
  *Autonomy* instead: they merge and deploy on green gates, ship additive migrations, and ask Jon
  about destructive ones, store submissions and money.
- **Who to ask.** "Hand it back to Jon", written into the workflow doc.

No developer using Astrid could see or change any of this. `Project` carried no agent
configuration at all. The policy turns these answers into stored data, and the core rules read it.
Every client and harness then gets the same answer, the way the Waiting lane works
(`astrid_rules::waiting`).

## 2. Shape

A **project** stores a `PolicyOverride`, and so may each **list**. Every key is optional, and an
absent key inherits:

```json
{
  "routing":    { "assignTo": "claude" | null, "agents": ["claude", "codex"] | null },
  "autonomy":   {
    "merge": "allowed" | "ask" | "never",
    "deploy": "…",
    "migrate": { "additive": "…", "destructive": "…" },
    "release": "…",
    "spend": "…"
  },
  "escalation": { "to": "listOwner" } | { "to": "taskCreator" } | { "to": "people", "userIds": ["…"] }
}
```

`resolve(project, list)` lays them over the default field by field: the default, then the
project's override, then the list's. The result is a `Policy` with every field decided. A list
that only says `autonomy.merge` inherits everything else. A list with no project has only its own
override. Under `routing`, `null` is a value meaning "any agent", so it overrides an inherited
agent. Only an absent key inherits.

Agents are named by **mailbox** (`claude`, `codex`, `copilot`), the same name the queue and the
claim use. Case and surrounding space are ignored. People are named by user id.

## 3. The default is today's behaviour (O2)

| | default |
|---|---|
| routing | `assignTo: null, agents: null`: any agent, and the first claim wins |
| merge, deploy, migrate (both kinds), release, spend | `never`: a person does it |
| escalation | `listOwner` |

That is the generated skill's behaviour, so nothing changes for anyone until someone stores a
policy. Astrid's own boards are not a special case. They run on one stored policy (§6).

## 4. The rule functions

| function | rules door | answers |
|---|---|---|
| `resolve(project, list)` | `orchestrationPolicy` | the effective `Policy` |
| `route(policy, agent)` | `orchestrationRoute` | `{ take, reason }`: may this agent take an unassigned Ready task on this list |
| `may(policy, agent, action, migration?)` | `orchestrationMay` | `{ decision, mustAsk, reason }` |
| `escalate(policy, listOwnerId, taskCreatorId?)` | `orchestrationEscalate` | `{ notify, handBack }` |

Each door request carries `project` and `list` (the stored overrides, either one absent or
`null`) plus that rule's own arguments. So the caller passes what it already stores, and the core
does the resolving.

**Routing (O3).** If `assignTo` names this agent, the agent takes the task (`routed`). If it names
another agent, this one leaves it (`routedElsewhere`). If it names nobody, an agent the list
admits takes it (`anyAdmittedAgent`) and the claim decides between admitted agents. An agent the
list does not admit leaves it (`notAdmitted`). A list admits the agents in `agents`, or every agent
when `agents` is unset. The routed agent is always admitted, so `assignTo` cannot name an agent
that is not allowed to work the list.

**Autonomy (O4).** `may` answers one of three values:

- `allowed`: do it.
- `ask`: send an approval request to the escalation target and wait.
- `never`: leave it for a person.

Two floors apply whatever a policy stores:

- A **destructive** migration is never answered more freely than an **additive** one. The answer
  is the stricter of the two (`destructiveFloor`).
- A migration whose kind is **not stated** is answered as destructive (`unclassifiedMigration`),
  because an unclassified change should be treated as one that cannot be undone. A caller
  classifies with astrid-web's `scripts/lib/destructive-migrations.ts` (AWTD-1680). The
  production workflow's own destructive check stays as the backstop.

An agent the list does not admit is answered `never` for every action (`notAdmitted`).

**Escalation (O5).** `notify` lists everyone who receives the question, in order and without
repeats. `handBack` is `waiting::hand_back` for the first of them: Inbox, open, assigned to that
person, which is R4 of the Waiting lane. `taskCreator` falls back to the list owner when the
creator is unknown. A `people` list that names nobody also falls back to the list owner, so a
question always reaches someone.

**A bad stored value is refused, not guessed.** `"merge": "sometimes"` is a `badRequest`, never
read as `allowed`.

## 5. What the policy does not decide

- **Gates.** A red gate is red whatever the policy says. `may(deploy) == allowed` means "deploy
  once the gates are green", not "deploy now". The gates belong to each repo.
- **The lanes.** Ready, Doing and Waiting are `astrid_rules::waiting` and the server's queue.
- **Per-platform loop scripts.** These are out of scope for the task. The mechanism is general.

## 6. Astrid's own policy (the fixture)

`orchestration.json`'s `astridPolicy` is `docs/FIXALL_WORKFLOW.md` → *Autonomy* stored as a
project policy:

```json
{
  "autonomy": {
    "merge": "allowed", "deploy": "allowed",
    "migrate": { "additive": "allowed", "destructive": "ask" },
    "release": "ask", "spend": "ask"
  },
  "escalation": { "to": "people", "userIds": ["jon"] }
}
```

The fixture replays it through the door. Merge and deploy are allowed. An additive migration
ships, while a destructive or unclassified one asks. A store release and spending ask. Questions
go to Jon. One more case shows a list asking to merge while its project allows it. `jon` stands in
for his user id. The web task stores the real policy.

## 7. Where it runs

On by default (`astrid-rules/orchestration`), so every native client has it through astrid-core,
and so does astrid-web's **Node** package (`astrid-rules-wasm/orchestration`,
`scripts/build-wasm.sh`). That is where the server asks it wherever an agent acts. It is **off in
the browser build**: no page decides what an agent may do, and the browser's size is a budget
(README, [BROWSER_DOOR.md](./BROWSER_DOOR.md)). The browser package refuses
`orchestration*` kinds by name, and `scripts/check-wasm-packages.mjs` asserts both halves. The
browser artifact is byte-for-byte what it was (268,013 brotli bytes). The Node package grows by
about 6 KB brotli, and the budget does not cover that package.

## 8. Decisions taken, and the ones left open

Under the autonomy rule (*a choice with a recommended option is not a question*) these are decided
here. Jon can override any of them, and each is a small change to the rules and the fixture.

1. **Decided (agent, 2026-11-15): autonomy is per policy, not per agent.** "May Claude merge but
   not Codex?" can be expressed today only by admitting different agents on different lists. A
   per-agent table is additive later (`autonomy.byAgent`), and nobody has asked for it.
2. **Decided (agent, 2026-11-15): a list override may loosen its project's policy as well as
   tighten it.** "Overridable per list" says nothing about direction. A project that wants a floor
   is a later field.
3. **Decided (agent, 2026-11-15): a task on several lists.** The queue, the claim and every
   action happen on one board (`listId`), so the caller asks with that board's override. If a
   caller has no board in hand, it asks for each list and takes the strictest answer
   (`Autonomy::stricter`). The core does not pick a list.
4. **Decided (agent, 2026-11-15): "which list a piece of work belongs on" stays outside the
   policy for now.** Today that is a filing convention. For example, astrid-core tasks go on the
   web or iOS board. Expressing it needs a work classifier, such as a repo or a path, that no
   caller sends yet. It is the routing half to add once a caller files through the core.
5. **Open: who may edit a policy.** The web task states it: owners and admins, through
   `lib/list-permissions.ts`. Answering it in the core would mean a `permissions` predicate. That
   should be added when a native client edits policies, which no client does yet.
