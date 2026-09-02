# Separate agent runs from terminal sessions

A Run represents an actor's bounded attempt to advance a Mission, while a
Session owns a durable process, PTY, terminal state, and output history. We
rejected the original one-Run-one-PTY model because ordinary shells may have no
agent run, headless runs may need no terminal, and one shell session may host
sequential runs. Mission tabs therefore list Sessions in their sidebar, and
waterfall Surfaces attach to Sessions rather than Runs.

