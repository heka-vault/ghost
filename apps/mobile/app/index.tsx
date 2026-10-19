import { Redirect } from "expo-router";

import { usePhone } from "../src/phone/store";

/** An unpaired phone can only pair. */
export default function Index() {
  const { pairing } = usePhone();
  return <Redirect href={pairing ? "/home" : "/pair"} />;
}
