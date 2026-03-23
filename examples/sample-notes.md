# Sample Notes for Semantic Search Demo

Copy these 10 notes into Apple Notes to see psyXe's BERT-powered semantic
search in action. Each note is designed so that **the most interesting search
queries use completely different words than the note itself** — demonstrating
that semantic search understands *meaning*, not just keywords.

## Quick Setup

```bash
# Load all 10 notes into Apple Notes automatically
./examples/load-sample-notes.sh

# Then ask your AI assistant: "rebuild the notes index"
# And try the demo queries at the bottom
```

Or create each note manually in Apple Notes (title = heading, body = text).

---

## Note 1: Morning Routine

Wake up at 6:30. French press coffee, two scoops of the Ethiopian blend from
Trader Joe's. Read for 20 minutes — currently working through "Thinking, Fast
and Slow" by Kahneman. Quick stretch routine from the PT, then shower and out
by 8.

## Note 2: Pasta Carbonara (Nonna's Version)

Guanciale, not bacon — this matters. Render it slowly over medium-low heat
until the fat is translucent. Egg yolks only (3 for a pound of rigatoni),
mixed with Pecorino Romano. Never add cream. The heat from the pasta does the
cooking. Toss off the burner to avoid scrambled eggs.

## Note 3: Tax Strategy Call with Sarah

Max out 401k before year-end ($23,000 limit). Harvest losses in the brokerage
account — the NVIDIA position has unrealized losses we can pair against the
Apple gains. Backdoor Roth conversion in January. Sarah mentioned the QBI
deduction might apply to the consulting income.

## Note 4: Backyard Deck Project

Pressure-treated lumber for the frame, composite decking boards on top (Trex
Transcend in "Spiced Rum"). Need 16 footers for the joists, 12-inch spacing.
Footings must be below frost line — contractor says 36 inches here. Budget:
$8,500 materials + $4,000 labor. HOA approval needed before breaking ground.

## Note 5: Barcelona Trip Planning

Direct flight on Delta, leaves JFK at 9pm Tuesday. Airbnb in El Born
neighborhood — walking distance to the Gothic Quarter. Must-see: Sagrada
Familia (book tickets online, sells out), Mercat de la Boqueria for lunch,
Park Güell for sunset. Day trip to Montserrat by train. Pack a light jacket,
evenings are cool in October.

## Note 6: 1:1 with Marcus — Performance Review Prep

His Q3 numbers are strong — 118% of quota. But the team feedback survey
flagged communication issues with the London office. Need to address the
timezone friction tactfully. Promotion to Senior is on track for March if Q4
holds. He wants to mentor one of the new hires — good sign of leadership
readiness.

## Note 7: Sophie's Birthday Party Ideas

She turns 7 on March 15th. Obsessed with marine biology lately — maybe an
ocean theme? Aquarium has a party room rental ($350 for 15 kids, includes a
behind-the-scenes tour). Alternative: art studio party at Color Me Mine. Cake
from Flour Bakery — she wants chocolate with purple frosting. Invite list: her
class (22 kids) minus the ones she says are "not her vibe" (lol).

## Note 8: Insomnia Research

Blue light exposure suppresses melatonin production by up to 50%. The
circadian rhythm is most sensitive between 9pm and midnight. Magnesium
glycinate (400mg) before bed showed improvement in REM latency in the
randomized trial. The CBT-I protocol — stimulus control, sleep restriction,
cognitive restructuring — has stronger evidence than medication for chronic
cases. Dr. Patel recommended the Huberman podcast episode on sleep hygiene.

## Note 9: Electric Vehicle Comparison

Tesla Model Y: 330 mi range, Supercharger network, $45k after incentives.
Hyundai Ioniq 5: 303 mi, faster 800V charging architecture, $41k. BMW iX
xDrive50: 324 mi, luxury interior, $65k. The federal tax credit ($7,500)
applies to all three. Home charger install quote: $1,200 for a 240V/48A
circuit in the garage. Break-even vs gas at ~45,000 miles.

## Note 10: Garden Seed Starting Schedule

Tomatoes and peppers: start indoors 8 weeks before last frost (Feb 15 here).
Basil and cucumbers: 4 weeks before. Direct sow after frost: beans, squash,
zucchini, sunflowers. The grow lights need to be 2-3 inches above seedlings,
16 hours on. Seed starting mix, NOT potting soil — too dense for germination.
Harden off over 7-10 days before transplanting.

---

## Demo Queries

Try these queries with your AI assistant after indexing the notes. None of
these queries share keywords with the matching note — the search works purely
on semantic understanding.

| Query | Expected Match | Why It Works |
|-------|---------------|--------------|
| "how to sleep better" | Insomnia Research | Finds melatonin, CBT-I, circadian rhythm — no "sleep better" in the note |
| "retirement savings" | Tax Strategy Call | Finds 401k, Roth conversion, loss harvesting — never says "retirement" |
| "kid's party planning" | Sophie's Birthday | Finds "she turns 7", aquarium, cake — never says "kid's party" |
| "home renovation budget" | Backyard Deck Project | Finds lumber, footings, $8,500 — never says "renovation" |
| "Italian cooking" | Pasta Carbonara | Finds guanciale, Pecorino, rigatoni — never says "Italian" |
| "European vacation" | Barcelona Trip | Finds JFK, Sagrada Familia, Airbnb — never says "European" or "vacation" |
| "employee development" | 1:1 with Marcus | Finds promotion, mentoring, leadership — never says "development" |
| "caffeine habits" | Morning Routine | Finds French press coffee, Ethiopian blend — never says "caffeine" |
| "comparing electric cars" | EV Comparison | Finds Tesla, Ioniq 5, range, tax credit — semantic and keyword overlap |
| "growing vegetables at home" | Garden Seed Schedule | Finds tomatoes, peppers, beans — never says "vegetables" |

### What to Look For

**Semantic search wins** when the query and the note use completely different
words but mean the same thing. A traditional keyword search for "retirement
savings" would return zero results because the Tax Strategy note never contains
those words. BERT embeddings understand that 401k, Roth conversion, and loss
harvesting are *about* retirement savings.

**Try a bad query too**: search for something completely unrelated like
"quantum physics" — you should see low relevance scores across all notes,
confirming the search isn't just returning random results.
