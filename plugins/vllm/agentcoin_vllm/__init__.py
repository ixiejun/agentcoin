"""AgentCoin TOPLOC plugin for vLLM (spec market/engine-plugin).

vLLM calls `register` in every process it starts. The plugin does nothing unless
`AGENTCOIN_TOPLOC_SOCKET` names the provider agent's local socket; then it checks its
preconditions when the model loads and, after every forward step, sends each request's top-k
final-layer activations to the provider. It never writes files and never logs content.
"""

import os

SOCKET_ENV = "AGENTCOIN_TOPLOC_SOCKET"


def register() -> None:
    """vLLM general-plugin entry point; re-entrant."""
    path = os.environ.get(SOCKET_ENV)
    if not path:
        return
    from . import runner

    runner.install(path)
