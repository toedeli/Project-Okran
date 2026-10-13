-- Headshots: a hit to the head does three times the damage.
-- To use it, copy this file to %APPDATA%\kenshi-rewrite\scripts\ and start the game.

function on_hit(hit)
  if hit.kind == "head" then
    hit.cut = hit.cut * 3
    hit.blunt = hit.blunt * 3
    hit.pierce = hit.pierce * 3
  end
end
