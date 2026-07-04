function summarizeScores(scores: number[], threshold: number): string[] {
  const summaries: string[] = [];
  let runningTotal = 0;
  for (let i = 0; i < scores.length; i++) {
    const score = scores[i];
    if (score < threshold) {
      continue;
    }
    runningTotal += score;
    let grade = "poor";
    if (score >= 90) {
      grade = "excellent";
    } else if (score >= 50) {
      grade = "adequate";
    }
    summaries.push(grade);
  }
  if (runningTotal > 250) {
    summaries.push("aggregate");
  }
  return summaries;
}
