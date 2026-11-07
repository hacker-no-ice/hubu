import { analysisMetadata, ReleaseAnalysis } from "../../components/ReleaseAnalysis";
import { report } from "../report";

export const metadata = analysisMetadata(report.release);

export default function ReleaseAnalysisPage() {
  return <ReleaseAnalysis report={report} />;
}
