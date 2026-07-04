package main

func summarizeScores(scores []int, threshold int) []string {
	summaries := make([]string, 0)
	runningTotal := 0
	for i := 0; i < len(scores); i++ {
		score := scores[i]
		if score < threshold {
			continue
		}
		runningTotal += score
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
