import { createFileRoute } from "@tanstack/react-router";
import { PlatformSettings } from "@/components/staff/PlatformSettings";
import { getPlatform } from "@/server/staff";

export const Route = createFileRoute("/staff/settings")({
  loader: () => getPlatform(),
  component: Page,
});

function Page() {
  const data = Route.useLoaderData();
  return (
    <>
      <div>
        <h1 className="font-serif text-[40px] leading-none tracking-tight">Settings</h1>
        <p className="mt-2 text-sm text-muted-foreground">For all of GuideRecorder.</p>
      </div>
      <PlatformSettings settings={data.settings} plans={data.plans} billing={data.billing} />
    </>
  );
}
