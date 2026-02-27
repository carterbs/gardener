scripts/ralph has grown into a full blown orchestrator but it is not mature enough. I see git errors, collisions with the backlog, hanging worktrees, all manner of things. I would like you to take a look at the state of the world - currently open worktrees. recently merged PRs. the source code itself. and look at how we can make the next iteration more robust. 

The new orchestrator should be writen in rust. It must have 100% test coverage.

Here are the core requirements:
* There must be a central backlog of tasks. Tasks should have differing levels of priority. npm run validate doesn't work on main = p0. Noticed an unmerged PR for a task that's on the backlog? P0. Implement new feature? p1. (You will need to clearly enumerate what makes something high priority or not)
* Upon starting the orchestrator should check: does a quality-graces doc exist? If not, we need to build out the infrastructure required to generate quality grades.
* Otherwise, we should look at the backlog.
* If there's nothing on the backlog, we should fill it based on docs/references/codex-agent-team-article.md. The goal is to make the repository simpler for agents to work inside of.
* The orchestrator must be able to to start N number of workers, each of which requests work from the orchestrator. The orchestrator gives the highest priority task to the agent in the order that they ask for work 
* A worker receives the task and spawns a coding agent to work on it. The coding agent is just a binary. The initially supported binaries are either `claude` or `codex` - but it should be trivial to add another one if needed. We'll launch them in non-interactive mode with sandboxes - see the existing implementation for guidance.
* The worker should proceed through a state machine, with each state having separate initial prompts. Users should be able to use different models at different states.
* States:
    * 1. UNDERSTAND: The coding agent receives the task description and categorizes the task type. Is this a task (e.g., get the PR merged, fix main)? Is this a feature? Is this a bug fix?
    * 2. IF task proceed to DOING:
    * 2. ELSE PLANNING
    * 3. DOING: Agent iterates for up to 100 turns
    * 4. GITTING: Agent commits and pushes
    * 5. REVIEWING: Agent reviews the PR in the worktree
    * IF agent returns suggestions to the orchestration, move to DOING. Cap iterations at 3.
    * 6. MERGING: Agent must get the PR mergeable (rebase on main, resolve merge conflicts)
    * 7. COMPLETE
* There should be a terminal UI that displays each worker's current state. Every time a worker's agent makes a tool call, it needs to be displayed in a friendly way. All tool calls and agent output should be logged to a jsonl file for debugging.