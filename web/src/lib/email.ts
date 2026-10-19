export interface Email {
  to: string;
  subject: string;
  text: string;
}

export type SendEmail = (email: Email) => Promise<void>;

/** Sends through Resend when configured, else logs (local development). */
export function emailSender(config: { resendApiKey?: string; from: string }): SendEmail {
  const { resendApiKey, from } = config;
  if (!resendApiKey) {
    return async ({ to, subject, text }) => {
      console.info(`[email] to=${to} subject=${JSON.stringify(subject)}\n${text}`);
    };
  }
  return async ({ to, subject, text }) => {
    const response = await fetch("https://api.resend.com/emails", {
      method: "POST",
      headers: { authorization: `Bearer ${resendApiKey}`, "content-type": "application/json" },
      body: JSON.stringify({ from, to, subject, text }),
    });
    if (!response.ok) {
      throw new Error(`Resend failed with ${response.status}: ${await response.text()}`);
    }
  };
}
