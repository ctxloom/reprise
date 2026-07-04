function summarizeScores(scores: number[], threshold: number): string[] {
  const summaries: string[] = [];
  let runningTotal = 0;
  for (const score of scores) {
    if (score < threshold) {
      continue;
    }
    runningTotal += score / 2;
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
