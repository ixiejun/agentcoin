> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-public-jobs

The public job queue of AgentCoin (m6-public-jobs; spec `market/public-jobs`): zero-stake
workers run evaluation, data cleaning and embedding units three at a time, commit to and reveal
small summaries, and a unit passes when two summaries agree. Canary units catch a colluding
majority. Workers are paid from public work emission; claimed rewards stay locked and slashable
for seven days.

The full description (parameters, calls, canaries and locking) follows with task 9.1 of the
change.
