#!/usr/bin/env bash
# load-sample-notes.sh — Create the sample notes in Apple Notes for demo purposes.
#
# Usage:
#   ./examples/load-sample-notes.sh              # Create in default "Notes" folder
#   ./examples/load-sample-notes.sh "Demo Notes"  # Create in a specific folder

set -euo pipefail

FOLDER="${1:-Notes}"

bold() { printf "\033[1m%s\033[0m\n" "$*"; }
green() { printf "\033[32m  ✓ %s\033[0m\n" "$*"; }
red() { printf "\033[31m  ✗ %s\033[0m\n" "$*"; }

bold "Creating sample notes in folder: $FOLDER"
echo ""

create_note() {
    local title="$1"
    local body="$2"
    local folder="$FOLDER"

    # Use osascript with stdin to avoid shell quoting issues
    local result
    result=$(osascript <<APPLESCRIPT
tell application "Notes"
    set targetFolder to missing value
    repeat with f in every folder
        if (name of f) = "$folder" then
            set targetFolder to f
            exit repeat
        end if
    end repeat
    if targetFolder is missing value then
        set targetFolder to default folder of default account
    end if
    set noteBody to "$body"
    make new note at targetFolder with properties {name:"$title", body:noteBody}
    return "OK"
end tell
APPLESCRIPT
    2>&1) || true

    if [[ "$result" == *"OK"* ]] || [[ "$result" == *"note id"* ]]; then
        green "$title"
    else
        red "$title — $result"
    fi
}

create_note "Morning Routine" \
"Wake up at 6:30. French press coffee, two scoops of the Ethiopian blend from Trader Joes. Read for 20 minutes — currently working through Thinking, Fast and Slow by Kahneman. Quick stretch routine from the PT, then shower and out by 8."

create_note "Pasta Carbonara (Nonnas Version)" \
"Guanciale, not bacon — this matters. Render it slowly over medium-low heat until the fat is translucent. Egg yolks only (3 for a pound of rigatoni), mixed with Pecorino Romano. Never add cream. The heat from the pasta does the cooking. Toss off the burner to avoid scrambled eggs."

create_note "Tax Strategy Call with Sarah" \
"Max out 401k before year-end (23000 limit). Harvest losses in the brokerage account — the NVIDIA position has unrealized losses we can pair against the Apple gains. Backdoor Roth conversion in January. Sarah mentioned the QBI deduction might apply to the consulting income."

create_note "Backyard Deck Project" \
"Pressure-treated lumber for the frame, composite decking boards on top (Trex Transcend in Spiced Rum). Need 16 footers for the joists, 12-inch spacing. Footings must be below frost line — contractor says 36 inches here. Budget: 8500 materials + 4000 labor. HOA approval needed before breaking ground."

create_note "Barcelona Trip Planning" \
"Direct flight on Delta, leaves JFK at 9pm Tuesday. Airbnb in El Born neighborhood — walking distance to the Gothic Quarter. Must-see: Sagrada Familia (book tickets online, sells out), Mercat de la Boqueria for lunch, Park Guell for sunset. Day trip to Montserrat by train. Pack a light jacket, evenings are cool in October."

create_note "1-on-1 with Marcus — Performance Review Prep" \
"His Q3 numbers are strong — 118% of quota. But the team feedback survey flagged communication issues with the London office. Need to address the timezone friction tactfully. Promotion to Senior is on track for March if Q4 holds. He wants to mentor one of the new hires — good sign of leadership readiness."

create_note "Sophies Birthday Party Ideas" \
"She turns 7 on March 15th. Obsessed with marine biology lately — maybe an ocean theme? Aquarium has a party room rental (350 for 15 kids, includes a behind-the-scenes tour). Alternative: art studio party at Color Me Mine. Cake from Flour Bakery — she wants chocolate with purple frosting. Invite list: her class (22 kids) minus the ones she says are not her vibe."

create_note "Insomnia Research" \
"Blue light exposure suppresses melatonin production by up to 50%. The circadian rhythm is most sensitive between 9pm and midnight. Magnesium glycinate (400mg) before bed showed improvement in REM latency in the randomized trial. The CBT-I protocol — stimulus control, sleep restriction, cognitive restructuring — has stronger evidence than medication for chronic cases. Dr. Patel recommended the Huberman podcast episode on sleep hygiene."

create_note "Electric Vehicle Comparison" \
"Tesla Model Y: 330 mi range, Supercharger network, 45k after incentives. Hyundai Ioniq 5: 303 mi, faster 800V charging architecture, 41k. BMW iX xDrive50: 324 mi, luxury interior, 65k. The federal tax credit (7500) applies to all three. Home charger install quote: 1200 for a 240V 48A circuit in the garage. Break-even vs gas at about 45000 miles."

create_note "Garden Seed Starting Schedule" \
"Tomatoes and peppers: start indoors 8 weeks before last frost (Feb 15 here). Basil and cucumbers: 4 weeks before. Direct sow after frost: beans, squash, zucchini, sunflowers. The grow lights need to be 2-3 inches above seedlings, 16 hours on. Seed starting mix, NOT potting soil — too dense for germination. Harden off over 7-10 days before transplanting."

echo ""
bold "Done! Created 10 sample notes."
echo ""
echo "Next steps:"
echo "  1. Ask your AI assistant: \"rebuild the notes index\""
echo "  2. Try: \"search my notes for retirement savings\""
echo "  3. See examples/sample-notes.md for more demo queries"
