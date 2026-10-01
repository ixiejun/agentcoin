"""AgentCoin TOPLOC plugin for vLLM (spec market/engine-plugin).

vLLM calls `register` in every process it starts. The plugin does nothing unless
`AGENTCOIN_TOPLOC_SOCKET` names a receiver's local socket; then it checks its preconditions
when the model loads and, after every forward step, sends top-k final-layer activations: in the
`prove` mode (the default, for a provider agent) each request's per step, in the `verify` mode
(`AGENTCOIN_TOPLOC_MODE=verify`, for an auditor's re-check) one segment per prefilled token row.
It never writes files and never logs content.
"""

import os

SOCKET_ENV = "AGENTCOIN_TOPLOC_SOCKET"
MODE_ENV = "AGENTCOIN_TOPLOC_MODE"


def register() -> None:
    """vLLM general-plugin entry point; re-entrant."""
    path = os.environ.get(SOCKET_ENV)
    if not path:
        return
    from . import runner

    runner.install(path, runner.parse_mode(os.environ.get(MODE_ENV)))
