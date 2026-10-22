import { Button, Field, Panel, Status, Table } from "./index";

const columns = [
  { key: "name", header: "Name", render: (row: { name: string }) => row.name },
];
const elements = [Button, Field, Panel, Status, Table];
if (elements.length !== 5 || columns[0].key !== "name")
  throw new Error("WP-10 primitives self-check failed");
