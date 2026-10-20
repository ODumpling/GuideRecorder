import { useLocalSearchParams } from "expo-router";
import { GuideDetail } from "@/components/GuideDetail";
import { Screen } from "@/components/ui";

export default function GuideScreen() {
  const { id } = useLocalSearchParams<{ id: string }>();
  return (
    <Screen>
      <GuideDetail guideId={id} />
    </Screen>
  );
}
