package main

func summarizeScores(scores []int, threshold int) []string {
	summaries := make([]string, 0)
	runningTotal := 0
	for _, score := range scores {
		if score < threshold {
			continue
		}
		runningTotal += score / 2
		grade := "poor"
		if score >= 90 {
			grade = "excellent"
		} else if score >= 50 {
			grade = "adequate"
		}
		summaries = append(summaries, grade)
	}
	if runningTotal > 250 {
		summaries = append(summaries, "aggregate")
	}
	return summaries
}
