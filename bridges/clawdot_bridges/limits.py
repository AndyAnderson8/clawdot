"""Rate limiting and duplicate suppression, so a noisy channel cannot make the speaker talk all day."""

from __future__ import annotations

import hashlib
import time
from collections import OrderedDict
from collections.abc import Callable


class TokenBucket:
    """Allows ``burst`` messages at once and ``per_minute`` over time. Drops are counted, never queued."""

    def __init__(self, per_minute: float, burst: int, clock: Callable[[], float] = time.monotonic) -> None:
        if per_minute <= 0 or burst < 1:
            raise ValueError("per_minute must be positive and burst at least 1")
        self.rate = per_minute / 60.0
        self.capacity = float(burst)
        self.tokens = float(burst)
        self.clock = clock
        self.last = clock()
        self.dropped = 0

    def allow(self) -> bool:
        now = self.clock()
        self.tokens = min(self.capacity, self.tokens + (now - self.last) * self.rate)
        self.last = now
        if self.tokens >= 1.0:
            self.tokens -= 1.0
            return True
        self.dropped += 1
        return False


class Dedupe:
    """Remembers what was sent in the last ``window`` seconds. Bounded in size."""

    def __init__(self, window: float = 120.0, capacity: int = 256, clock: Callable[[], float] = time.monotonic) -> None:
        self.window = window
        self.capacity = capacity
        self.clock = clock
        self.seen: OrderedDict[str, float] = OrderedDict()

    def is_repeat(self, *parts: str) -> bool:
        """True if the same parts were seen within the window; otherwise remembers them."""
        now = self.clock()
        while self.seen and (len(self.seen) > self.capacity or now - next(iter(self.seen.values())) > self.window):
            self.seen.popitem(last=False)
        key = hashlib.sha256("\x1f".join(parts).encode()).hexdigest()
        if key in self.seen and now - self.seen[key] <= self.window:
            return True
        self.seen[key] = now
        self.seen.move_to_end(key)
        return False
