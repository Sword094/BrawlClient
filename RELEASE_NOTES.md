## What's new in 0.2.9

**Fixes around ranked season resets**
- The elo graph's change and low/high no longer count the reset. Before, a 7 day, 30 day or all time view that crossed a new season counted the drop as a loss and showed last season's final elo as the high. Those numbers now cover the current season only, and the graph says so.
- A reset can no longer be counted as the result of a single game, and the season banner ignores a leftover reading from the old season when working out the new season's start and peak.
- The banner's "Won" and "Lost" tiles are now labelled "Won in games" and "Lost in games", with a note that they add up each game's change and aren't your net.
