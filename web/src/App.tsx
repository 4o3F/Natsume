const capabilities = [
  "Single authoritative seat/account/password CSV",
  "Device-only manual or policy-approved enrollment",
  "Explicit state synchronization with Gateway PKI",
  "Human-only secret synchronization",
  "Desktop session lock / unlock",
  "Visual Gateway status",
];

export function App() {
  return (
    <main>
      <h1>Natsume V2 Preparation Center</h1>
      <p>
        Architecture blueprint for the v2.5 single-contest workstation control
        boundary.
      </p>
      <ul>
        {capabilities.map((capability) => (
          <li key={capability}>{capability}</li>
        ))}
      </ul>
    </main>
  );
}
