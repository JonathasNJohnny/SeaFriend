param(
    [string]$ApiUrl = "http://127.0.0.1:3100"
)

$ErrorActionPreference = "Stop"
$sizes = @(2, 4, 6, 8, 10)

foreach ($size in $sizes) {
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    $room = Invoke-RestMethod -Method Post -Uri "$ApiUrl/rooms" -ContentType "application/json" -Body (@{ display_name = "CapacityHost-$size" } | ConvertTo-Json)
    for ($index = 2; $index -le $size; $index++) {
        Invoke-RestMethod -Method Post -Uri "$ApiUrl/rooms/join" -ContentType "application/json" -Body (@{ code = $room.access_code; display_name = "CapacityGuest-$size-$index" } | ConvertTo-Json) | Out-Null
    }
    $timer.Stop()
    [pscustomobject]@{
        participants = $size
        elapsed_ms = $timer.ElapsedMilliseconds
        room_id = $room.room.id
    }
}
