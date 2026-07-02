fun summarizeScores(scores: List<Int>, threshold: Int): List<String> {
    val summaries = mutableListOf<String>()
    var runningTotal = 0
    for (score in scores) {
        if (score < threshold) {
            continue
        }
        runningTotal += score
        var grade = "poor"
        if (score >= 90) {
            grade = "excellent"
        } else if (score >= 50) {
            grade = "adequate"
        }
        summaries.add(grade)
    }
    if (runningTotal > 250) {
        summaries.add("aggregate")
    }
    return summaries
}
