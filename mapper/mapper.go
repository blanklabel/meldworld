package mapper

import (
	"encoding/json"
	"fmt"
)

// Size of a map
type Dimension struct {
	Height int
	Width  int
}

// Container for the jason
type MapObj struct {
	Map Dimension
}

func main() {
	var dict string = `{
	"mapper": {
		"height": 200,
		"width": 200
	}
    }`
	fmt.Println(dict)

	jo := MapObj{}
	json.Unmarshal([]byte(dict), &jo)
	fmt.Println(jo)

}
