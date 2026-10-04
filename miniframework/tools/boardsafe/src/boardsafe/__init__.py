"""boardsafe: put the right firmware on the right board, and prove it.

An app describes its physical boards once, in a registry (a Python file with
a ``BOARDS`` dict of :class:`boardsafe.boards.Board`). Every step reads its
board from there:

- :mod:`boardsafe.image` refuses an image built for another chip, another
  board of the app, or too large for the app partition;
- :mod:`boardsafe.flash` updates only the application partition, after
  checking the connected chip, its MAC and its exact partition table;
- :mod:`boardsafe.probe` checks a live board over strict TLS: hostname,
  identity, every bundled asset byte for byte, and the CA it serves.

Nothing here erases a chip, rewrites a partition table or touches a data
partition. First installs are deliberately a separate, explicit tool.

The library uses only the standard library; esptool and pyserial are needed
only to talk to a board.
"""
