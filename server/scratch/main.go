package main

import (
	"encoding/json"
	"fmt"
)

var JSON = []byte(`
{
"type": "something",
"payload": {
    "55A1EA1D-D363-4F3A-AD29-0898C92F59BC": {
      "owner": "333333333",
      "ID": "55A1EA1D-D363-4F3A-AD29-0898C92F59BC",
      "name": "guy",
      "statuses": [],
      "full_hp": 27,
      "c_hp": 27,
      "phy_def": 13,
      "phy_atk": 13,
      "speed": 1,
      "coordinates": {
        "x": 9,
        "y": 1
      },
      "destination": {
        "x": 2,
        "y": 3
      }
    },
    "F64CB41D-4FAE-48A1-93A9-C5B9C3DE50FD": {
      "owner": "333333333",
      "ID": "F64CB41D-4FAE-48A1-93A9-C5B9C3DE50FD",
      "name": "player4",
      "statuses": [],
      "full_hp": 2,
      "c_hp": 1,
      "phy_def": 2,
      "phy_atk": 2,
      "speed": 1,
      "coordinates": {
        "x": 7,
        "y": 1
      },
      "destination": {
        "x": 6,
        "y": 7
      }
    }
  }
}`)

type Cords struct {
	X, Y int
}

type ModelType struct {
	MsgType string `json:"type"`
}

type Ent struct {
	ID          string
	Owner       string
	Name        string
	Full_hp     int
	C_hp        int
	Phy_def     int
	Phy_atk     int
	Speed       int // tiles per tick
	Coordinates Cords
	Destination Cords
}

type Message struct {
	MsgType string `json:"type"`
	Payload *json.RawMessage
}

type Entities map[string]Ent

func main() {
	fmt.Println(JSON)
	dat := Message{}

	json.Unmarshal(JSON, &dat)
	//fmt.Println("What we got", dat)
	fmt.Println("Message type: ", dat.MsgType)
	//fmt.Println("What we got", *dat.Payload)

	ents := Entities{}
	json.Unmarshal(*dat.Payload, &ents)

	fmt.Println(ents)
	fmt.Println("A specific ent: ", ents["55A1EA1D-D363-4F3A-AD29-0898C92F59BC"].Name)

}
