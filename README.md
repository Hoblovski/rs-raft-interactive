# Interactive Raft
The Raft protocol, as a transition system, interactively.

May be extended to some kind of model checking tool.
May be generalized to a TLA+ like framework.

----------------

# Usage
## Run raft from scratch
```
cargo run
# or better (has history recording in hist.txt)
./init -
```

## Fig. 8 in raft paper
```
./init fig8.txt
```

See states when the next step is
* 55: the entry [idx=1,term=1] has been replicated by 0 (leader at 3) to a majority 0, 1, 2
* 97: the previous entry which was replicated to a majority was overwritten

The results say that entries do not commit simply by getting replicated to a majority.

----------------

# Description of init directives
See fig8.txt.

Extras:
* `// ...` comments
* `@quit` quit directives and enter interactive mode

----------------

# TODO
dropping messages

duplicating messages

snapshot

reconfiguration
